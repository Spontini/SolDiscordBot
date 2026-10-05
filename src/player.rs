use crate::{
    model::{self, Curve, Media, OUTPUT_GAIN, Position},
    resolver::Resolver,
};
use anyhow::{Result, bail};
use songbird::{
    Call,
    input::{Input, RawAdapter, codecs, core::io::MediaSource},
    tracks::TrackHandle,
};
use std::{
    collections::VecDeque,
    io::{Read, Seek, SeekFrom},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, sync_channel},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

// Holds the decoder slot until its process has been killed and reaped.
struct Decoder {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    frames: StdMutex<Receiver<Vec<u8>>>,
    frame: Vec<u8>,
    offset: usize,
}
impl Decoder {
    fn buffered(
        mut child: Child,
        permit: OwnedSemaphorePermit,
    ) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let mut stdout = child.stdout.take().expect("decoder stdout was piped");
        // 500 stereo 20 ms float32 frames = 10 seconds = 3,840,000 bytes.
        let (sender, frames) = sync_channel(500);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let mut ready_tx = Some(ready_tx);
            let mut count = 0;
            loop {
                let mut frame = vec![0; 7680];
                let mut filled = 0;
                while filled < frame.len() {
                    match stdout.read(&mut frame[filled..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => filled += n,
                    }
                }
                if filled == 0 {
                    break;
                }
                frame.truncate(filled);
                if sender.send(frame).is_err() {
                    break;
                }
                count += 1;
                if count == 25 {
                    if let Some(ready) = ready_tx.take() {
                        let _ = ready.send(());
                    }
                }
                if filled < 7680 {
                    break;
                }
            }
            if count > 0 {
                if let Some(ready) = ready_tx.take() {
                    let _ = ready.send(());
                }
            }
        });
        (
            Self {
                child: Some(child),
                permit: Some(permit),
                frames: StdMutex::new(frames),
                frame: Vec::new(),
                offset: 0,
            },
            ready_rx,
        )
    }
}
impl Read for Decoder {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.offset == self.frame.len() {
            match self.frames.get_mut().expect("PCM receiver lock").recv() {
                Ok(frame) => {
                    self.frame = frame;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let len = buf.len().min(self.frame.len() - self.offset);
        buf[..len].copy_from_slice(&self.frame[self.offset..self.offset + len]);
        self.offset += len;
        Ok(len)
    }
}
impl Seek for Decoder {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}
impl MediaSource for Decoder {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        let child = self.child.take();
        let permit = self.permit.take();
        let cleanup = move || {
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            drop(permit);
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(cleanup);
        } else {
            cleanup();
        }
    }
}

async fn prepare(
    resolver: Resolver,
    decoders: Arc<Semaphore>,
    mut media: Media,
) -> Result<(Media, Input)> {
    let stream = resolver.stream(&media).await?;
    media.duration = stream.duration;
    let permit = decoders.acquire_owned().await?;
    let mut command = Command::new("ffmpeg");
    command.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-rw_timeout",
        "15000000",
        "-protocol_whitelist",
        "http,https,tcp,tls,crypto",
    ]);
    if !stream.headers.is_empty() {
        command.arg("-headers").arg(&stream.headers);
    }
    let child = command
        .args([
            "-i",
            &stream.url,
            "-vn",
            "-ac",
            "2",
            "-ar",
            "48000",
            "-f",
            "f32le",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let (decoder, ready) = Decoder::buffered(child, permit);
    tokio::time::timeout(Duration::from_secs(20), ready).await??;
    let input: Input = RawAdapter::new(decoder, 48000, 2).into();
    // Songbird parses the header on a blocking worker, never on the command task.
    let input = input
        .make_playable_async(codecs::get_codec_registry(), codecs::get_probe())
        .await?;
    Ok((media, input))
}

struct Playing {
    media: Media,
    handle: TrackHandle,
}
struct Pending {
    media: Media,
    task: JoinHandle<Result<(Media, Input)>>,
}
struct State {
    call: Arc<Mutex<Call>>,
    queue: VecDeque<Media>,
    current: Option<Playing>,
    incoming: Option<Playing>,
    pending: Option<Pending>,
    primed: Option<(Media, Input)>,
    paused: bool,
    fade_enabled: bool,
    fade_seconds: f64,
    active_fade_seconds: f64,
    curve: Curve,
    last_error: Option<String>,
    shutdown: bool,
}

#[derive(Clone)]
pub struct Player {
    state: Arc<Mutex<State>>,
    pub generation: Arc<AtomicU64>,
    resolver: Resolver,
    decoders: Arc<Semaphore>,
}

impl Player {
    pub fn new(call: Arc<Mutex<Call>>, resolver: Resolver, decoders: Arc<Semaphore>) -> Self {
        let state = Arc::new(Mutex::new(State {
            call,
            queue: VecDeque::new(),
            current: None,
            incoming: None,
            pending: None,
            primed: None,
            paused: false,
            fade_enabled: false,
            fade_seconds: 5.0,
            active_fade_seconds: 5.0,
            curve: Curve::Linear,
            last_error: None,
            shutdown: false,
        }));
        let player = Self {
            state: state.clone(),
            generation: Arc::new(AtomicU64::new(0)),
            resolver,
            decoders,
        };
        let worker = player.clone();
        let weak = Arc::downgrade(&state);
        // The worker owns a Weak state so disconnect releases the guild allocation.
        let resolver = worker.resolver.clone();
        let decoders = worker.decoders.clone();
        drop(worker);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(20));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else { break };
                let mut state = state.lock().await;
                if state.shutdown {
                    break;
                }
                state.tick(&resolver, &decoders).await;
            }
        });
        player
    }
    pub fn epoch(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
    pub async fn enqueue(
        &self,
        batch: Vec<Media>,
        position: Position,
        epoch: u64,
    ) -> Result<usize> {
        let mut state = self.state.lock().await;
        if self.epoch() != epoch || state.shutdown {
            bail!("This request was cancelled by a newer playback command.");
        }
        let reserved = usize::from(state.pending.is_some()) + usize::from(state.primed.is_some());
        if state.queue.len() + reserved + batch.len() > model::MAX_QUEUE {
            bail!("Queue limit is 200 tracks.");
        }
        let count = batch.len();
        if matches!(position, Position::Next | Position::Now) {
            state.restore_prepared();
        }
        if position == Position::Now {
            self.generation.fetch_add(1, Ordering::SeqCst);
            state.stop_tracks();
            state.paused = false;
        }
        model::enqueue(&mut state.queue, batch, position).map_err(anyhow::Error::msg)?;
        state.last_error = None;
        Ok(count)
    }
    pub async fn pause(&self) -> Result<String> {
        let mut state = self.state.lock().await;
        if state.current.is_none() {
            bail!("Nothing is playing.");
        }
        state.paused = !state.paused;
        for playing in [&state.current, &state.incoming].into_iter().flatten() {
            if state.paused {
                playing.handle.pause()?;
            } else {
                playing.handle.play()?;
            }
        }
        Ok(if state.paused { "Paused." } else { "Resumed." }.into())
    }
    pub async fn skip(&self) -> String {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().await;
        if state.current.is_none() {
            state.cancel_prepared();
        }
        if let Some(old) = state.current.take() {
            let _ = old.handle.stop();
        }
        state.current = state.incoming.take();
        if let Some(current) = &state.current {
            let _ = current.handle.set_volume(OUTPUT_GAIN);
            if state.paused {
                let _ = current.handle.pause();
            }
        }
        "Skipped.".into()
    }
    pub async fn stop(&self, disconnect: bool) -> String {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().await;
        state.stop_tracks();
        state.cancel_prepared();
        state.queue.clear();
        state.paused = false;
        state.shutdown = disconnect;
        if disconnect {
            "Disconnected."
        } else {
            "Stopped and cleared the queue."
        }
        .into()
    }
    pub async fn crossfade(&self, enabled: bool, seconds: f64, curve: Curve) -> Result<String> {
        if !(3.0..=10.0).contains(&seconds) {
            bail!("Crossfade duration must be between 3 and 10 seconds.");
        }
        let mut state = self.state.lock().await;
        if state.incoming.is_some() {
            bail!("Wait for the current transition to finish before changing crossfade settings.");
        }
        state.fade_enabled = enabled;
        state.fade_seconds = seconds;
        state.curve = curve;
        Ok(format!(
            "Crossfade {} ({seconds:.0}s, {curve:?}).",
            if enabled { "enabled" } else { "disabled" }
        ))
    }
    pub async fn describe(&self, queue: bool) -> String {
        let state = self.state.lock().await;
        let mut out = String::new();
        if let Some(current) = &state.current {
            out.push_str(&format!(
                "Playing: {} — {}{}\n",
                current.media.title,
                current.media.artist,
                if state.paused { " (paused)" } else { "" }
            ));
        } else {
            out.push_str("Nothing is playing.\n");
        }
        if let Some(next) = &state.incoming {
            out.push_str(&format!("Overlapping: {}\n", next.media.title));
        }
        if queue {
            let reserved = state
                .pending
                .as_ref()
                .map(|p| &p.media)
                .or_else(|| state.primed.as_ref().map(|p| &p.0));
            for (i, media) in reserved
                .into_iter()
                .chain(state.queue.iter())
                .take(10)
                .enumerate()
            {
                out.push_str(&format!("{}. {}\n", i + 1, media.title));
            }
            out.push_str(&format!(
                "{} queued tracks.",
                state.queue.len() + usize::from(reserved.is_some())
            ));
        }
        if let Some(error) = &state.last_error {
            out.push_str(&format!("\nLast playback error: {error}"));
        }
        out.chars().take(1800).collect()
    }
}

impl State {
    fn stop_tracks(&mut self) {
        for playing in [self.current.take(), self.incoming.take()]
            .into_iter()
            .flatten()
        {
            let _ = playing.handle.stop();
        }
    }
    fn cancel_prepared(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.task.abort();
        }
        self.primed = None;
    }
    fn restore_prepared(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.task.abort();
            self.queue.push_front(pending.media);
        }
        if let Some((media, _)) = self.primed.take() {
            self.queue.push_front(media);
        }
    }
    fn start_preparing(&mut self, resolver: &Resolver, decoders: &Arc<Semaphore>) {
        if self.pending.is_none() && self.primed.is_none() {
            if let Some(media) = self.queue.pop_front() {
                let task = tokio::spawn(prepare(resolver.clone(), decoders.clone(), media.clone()));
                self.pending = Some(Pending { media, task });
            }
        }
    }
    async fn attach(&mut self, media: Media, input: Input, volume: f32) -> Playing {
        let handle = self
            .call
            .lock()
            .await
            .play(songbird::tracks::Track::new(input).volume(volume));
        if self.paused {
            let _ = handle.pause();
        }
        Playing { media, handle }
    }
    async fn tick(&mut self, resolver: &Resolver, decoders: &Arc<Semaphore>) {
        if self.pending.as_ref().is_some_and(|p| p.task.is_finished()) {
            let pending = self.pending.take().unwrap();
            match pending.task.await {
                Ok(Ok(prepared)) => self.primed = Some(prepared),
                _ => {
                    self.last_error = Some(
                        "Could not prepare the next track; it was removed from the queue.".into(),
                    );
                    tracing::warn!("media preparation failed");
                }
            }
        }
        let (current_info, lost_handle) = if let Some(current) = &self.current {
            match tokio::time::timeout(Duration::from_millis(100), current.handle.get_info()).await
            {
                Ok(Ok(info)) => (Some(info), false),
                Ok(Err(_)) => (None, true),
                Err(_) => (None, false),
            }
        } else {
            (None, false)
        };
        // A dropped driver handle also means the current source is gone.
        if self.current.is_some()
            && (lost_handle || current_info.as_ref().is_some_and(|s| s.playing.is_done()))
        {
            self.current = None;
            self.current = self.incoming.take();
            if let Some(current) = &self.current {
                let _ = current.handle.set_volume(OUTPUT_GAIN);
                return;
            }
        }
        if self.current.is_none() {
            if let Some((media, input)) = self.primed.take() {
                self.current = Some(self.attach(media, input, OUTPUT_GAIN).await);
            } else {
                self.start_preparing(resolver, decoders);
            }
            return;
        }
        if self.paused {
            return;
        }
        if let Some(incoming) = &self.incoming {
            let incoming_info =
                tokio::time::timeout(Duration::from_millis(100), incoming.handle.get_info()).await;
            if matches!(&incoming_info, Ok(Err(_))) {
                self.incoming = None;
                if let Some(current) = &self.current {
                    let _ = current.handle.set_volume(OUTPUT_GAIN);
                }
                self.last_error =
                    Some("Incoming crossfade source failed; outgoing volume restored.".into());
                return;
            }
            if let Ok(Ok(info)) = incoming_info {
                if info.playing.is_done() {
                    self.incoming = None;
                    if let Some(current) = &self.current {
                        let _ = current.handle.set_volume(OUTPUT_GAIN);
                    }
                    self.last_error =
                        Some("Incoming crossfade track ended before promotion.".into());
                    return;
                }
                let t = info.play_time.as_secs_f64() / self.active_fade_seconds;
                let (out_gain, in_gain) = model::gains(self.curve, t);
                if let Some(current) = &self.current {
                    let _ = current.handle.set_volume(out_gain);
                }
                let _ = incoming.handle.set_volume(in_gain);
                if t >= 1.0 {
                    if let Some(old) = self.current.take() {
                        let _ = old.handle.stop();
                    }
                    self.current = self.incoming.take();
                }
            }
            return;
        }
        if let (Some(current), Some(info)) = (&self.current, current_info) {
            if let Some(duration) = current.media.duration {
                let remaining = duration - info.position.as_secs_f64();
                if remaining <= self.fade_seconds + 3.0 {
                    self.start_preparing(resolver, decoders);
                }
                // A late source or short/unknown track advances normally; never fake overlap.
                if self.fade_enabled && remaining >= 3.0 && remaining <= self.fade_seconds {
                    if let Some((media, input)) = self.primed.take() {
                        self.active_fade_seconds = remaining.min(self.fade_seconds);
                        self.incoming = Some(self.attach(media, input, 0.0).await);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn media() -> Media {
        Media {
            title: "Fixture".into(),
            artist: "Fixture".into(),
            url: "https://www.youtube.com/watch?v=x".into(),
            duration: Some(60.0),
            verified: false,
        }
    }
    fn player() -> Player {
        let call = Call::standalone(
            serenity::all::GuildId::new(1),
            serenity::all::UserId::new(2),
        );
        Player::new(
            Arc::new(Mutex::new(call)),
            Resolver::default(),
            Arc::new(Semaphore::new(2)),
        )
    }
    #[tokio::test]
    async fn stopped_search_cannot_revive_playback() {
        let player = player();
        let old_epoch = player.epoch();
        player.stop(false).await;
        assert!(
            player
                .enqueue(vec![media()], Position::End, old_epoch)
                .await
                .is_err()
        );
        assert!(player.state.lock().await.queue.is_empty());
        player.stop(true).await;
    }
    #[tokio::test]
    async fn stop_aborts_preparation_and_clears_reserved_queue() {
        let player = player();
        let task = tokio::spawn(std::future::pending::<Result<(Media, Input)>>());
        let abort = task.abort_handle();
        {
            let mut state = player.state.lock().await;
            state.pending = Some(Pending {
                media: media(),
                task,
            });
            state.queue.push_back(media());
        }
        player.stop(true).await;
        tokio::task::yield_now().await;
        let state = player.state.lock().await;
        assert!(state.pending.is_none());
        assert!(state.queue.is_empty());
        assert!(state.shutdown);
        assert!(abort.is_finished());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn decoder_slot_returns_after_process_cleanup() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().acquire_owned().await.unwrap();
        let child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (decoder, _ready) = Decoder::buffered(child, permit);
        drop(decoder);
        let recovered = tokio::time::timeout(Duration::from_secs(3), slots.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(recovered);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn short_pcm_source_becomes_ready_and_preserves_bytes() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.acquire_owned().await.unwrap();
        let child = Command::new("python3")
            .args([
                "-c",
                "import sys; sys.stdout.buffer.write(bytes(range(256))*60)",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (mut decoder, ready) = Decoder::buffered(child, permit);
        tokio::time::timeout(Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
        let bytes = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            decoder.read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        assert_eq!(bytes, (0..60).flat_map(|_| 0..=255u8).collect::<Vec<_>>());
    }
}
