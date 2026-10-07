use serenity::{
    all::{
        CommandInteraction, CreateActionRow, CreateAllowedMentions, CreateInteractionResponse,
        CreateInteractionResponseMessage, EditInteractionResponse,
    },
    http::Http,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

const IDENTITY_TTL: Duration = Duration::from_secs(300);
const MAX_IDENTITIES: usize = 128;

struct Identity {
    checked: Instant,
    name: String,
    revision: u64,
}
#[derive(Default)]
struct Identities {
    entries: HashMap<u64, Identity>,
    revision: u64,
}
#[derive(Default)]
pub(crate) struct IdentityCache(Mutex<Identities>);

pub(crate) struct JoinCancellation {
    pending: Mutex<Option<(u64, u64, u64)>>,
    revision: watch::Sender<u64>,
}
impl Default for JoinCancellation {
    fn default() -> Self {
        Self {
            pending: Mutex::new(None),
            revision: watch::channel(0).0,
        }
    }
}
pub(crate) struct JoinReservation {
    owner: Arc<JoinCancellation>,
    revision: u64,
    receiver: watch::Receiver<u64>,
}
impl JoinCancellation {
    pub(crate) fn begin(self: &Arc<Self>, guild: u64, channel: u64) -> JoinReservation {
        let mut pending = self.pending.lock().expect("pending join lock");
        self.revision.send_modify(|n| *n = n.wrapping_add(1));
        let revision = *self.revision.borrow();
        *pending = Some((guild, channel, revision));
        JoinReservation {
            owner: self.clone(),
            revision,
            receiver: self.revision.subscribe(),
        }
    }
    pub(crate) fn cancel(&self, guild: u64, channel: u64) -> bool {
        let pending = self.pending.lock().expect("pending join lock");
        if pending
            .as_ref()
            .is_some_and(|(g, c, _)| *g == guild && *c == channel)
        {
            self.revision.send_modify(|n| *n = n.wrapping_add(1));
            true
        } else {
            false
        }
    }
}
impl JoinReservation {
    pub(crate) async fn cancelled(&mut self) {
        loop {
            let current = *self.receiver.borrow_and_update();
            if current != self.revision {
                return;
            }
            if self.receiver.changed().await.is_err() {
                return;
            }
        }
    }
    // Atomically publish the player against cancellation. Called only with a
    // short-lived active-state guard, never with a network future in the closure.
    pub(crate) fn commit(self, publish: impl FnOnce()) -> bool {
        let mut pending = self.owner.pending.lock().expect("pending join lock");
        if pending
            .as_ref()
            .is_some_and(|(_, _, n)| *n == self.revision)
            && *self.receiver.borrow() == self.revision
        {
            publish();
            *pending = None;
            true
        } else {
            false
        }
    }
}
impl Drop for JoinReservation {
    fn drop(&mut self) {
        let mut pending = self.owner.pending.lock().expect("pending join lock");
        if pending
            .as_ref()
            .is_some_and(|(_, _, n)| *n == self.revision)
        {
            *pending = None;
        }
    }
}

impl IdentityCache {
    // Return cached/fallback identity immediately and reserve one background
    // refresh. No HTTP future is awaited by the caller or under this lock.
    pub(crate) fn lookup(
        &self,
        guild: u64,
        fallback: String,
        now: Instant,
    ) -> (String, Option<u64>) {
        let mut cache = self.0.lock().expect("identity cache lock");
        if let Some(entry) = cache.entries.get(&guild) {
            if now.saturating_duration_since(entry.checked) < IDENTITY_TTL {
                return (entry.name.clone(), None);
            }
        }
        let name = cache
            .entries
            .get(&guild)
            .map_or(fallback, |e| e.name.clone());
        cache
            .entries
            .retain(|_, e| now.saturating_duration_since(e.checked) < IDENTITY_TTL);
        if cache.entries.len() >= MAX_IDENTITIES {
            cache.entries.clear();
        }
        cache.revision = cache.revision.wrapping_add(1);
        let revision = cache.revision;
        cache.entries.insert(
            guild,
            Identity {
                checked: now,
                name: name.clone(),
                revision,
            },
        );
        (name, Some(revision))
    }

    pub(crate) fn finish(&self, guild: u64, revision: u64, name: String) {
        let mut cache = self.0.lock().expect("identity cache lock");
        if let Some(entry) = cache.entries.get_mut(&guild) {
            if entry.revision == revision {
                entry.name = name;
            }
        }
    }

    pub(crate) fn update(&self, guild: u64, name: String) {
        let mut cache = self.0.lock().expect("identity cache lock");
        if cache.entries.len() >= MAX_IDENTITIES && !cache.entries.contains_key(&guild) {
            cache.entries.clear();
        }
        cache.revision = cache.revision.wrapping_add(1);
        let revision = cache.revision;
        cache.entries.insert(
            guild,
            Identity {
                checked: Instant::now(),
                name,
                revision,
            },
        );
    }
}

// Borrow the pinned work future: a missed fast-response budget must not cancel
// a command already waiting for a player lock or a disconnect operation.
pub(crate) async fn initial_result<F: Future>(work: Pin<&mut F>) -> Option<F::Output> {
    tokio::time::timeout(Duration::from_millis(20), work)
        .await
        .ok()
}

pub(crate) async fn send_reply(
    command: &CommandInteraction,
    http: &Http,
    content: String,
    components: Vec<CreateActionRow>,
    deferred: bool,
) -> bool {
    if deferred {
        command
            .edit_response(
                http,
                EditInteractionResponse::new()
                    .content(content)
                    .components(components)
                    .allowed_mentions(CreateAllowedMentions::new()),
            )
            .await
            .is_ok()
    } else {
        command
            .create_response(
                http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .content(content)
                        .components(components)
                        .allowed_mentions(CreateAllowedMentions::new()),
                ),
            )
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_identity_returns_without_refresh_and_deduplicates_requests() {
        let cache = IdentityCache::default();
        let now = Instant::now();
        let (name, refresh) = cache.lookup(1, "Application".into(), now);
        assert_eq!(name, "Application");
        assert!(refresh.is_some());
        // Simulate a stalled HTTP refresh: subsequent commands still have a name.
        assert_eq!(cache.lookup(1, "Other".into(), now), (name, None));
    }

    #[test]
    fn late_http_refresh_cannot_overwrite_a_nickname_event() {
        let cache = IdentityCache::default();
        let (_, refresh) = cache.lookup(1, "Application".into(), Instant::now());
        cache.update(1, "New nickname".into());
        cache.finish(1, refresh.unwrap(), "Old nickname".into());
        assert_eq!(
            cache.lookup(1, "Application".into(), Instant::now()).0,
            "New nickname"
        );
    }

    #[test]
    fn expired_identity_remains_available_during_refresh() {
        let cache = IdentityCache::default();
        let now = Instant::now();
        let (_, refresh) = cache.lookup(1, "Application".into(), now);
        cache.finish(1, refresh.unwrap(), "Nickname".into());
        let later = now + IDENTITY_TTL;
        let (name, next) = cache.lookup(1, "Application".into(), later);
        assert_eq!(name, "Nickname");
        assert!(next.is_some());
        cache.finish(1, refresh.unwrap(), "Late old value".into());
        assert_eq!(cache.lookup(1, "Application".into(), later).0, name);
    }

    #[test]
    fn identity_cache_stays_bounded() {
        let cache = IdentityCache::default();
        for guild in 1..=1000 {
            cache.lookup(guild, "Application".into(), Instant::now());
            assert!(cache.0.lock().unwrap().entries.len() <= MAX_IDENTITIES);
        }
    }

    #[tokio::test]
    async fn slow_command_survives_the_initial_response_deadline() {
        let (send, recv) = tokio::sync::oneshot::channel();
        let work = async { recv.await.unwrap() };
        tokio::pin!(work);
        assert!(initial_result(work.as_mut()).await.is_none());
        send.send("Disconnected").unwrap();
        assert_eq!(work.await, "Disconnected");
    }

    #[tokio::test]
    async fn ready_command_needs_no_deferred_response() {
        let work = async { "Pong" };
        tokio::pin!(work);
        assert_eq!(initial_result(work.as_mut()).await, Some("Pong"));
    }

    #[tokio::test]
    async fn disconnect_cancels_a_stalled_join_only_in_the_same_voice_channel() {
        let joins = Arc::new(JoinCancellation::default());
        let mut pending = joins.begin(1, 2);
        assert!(!joins.cancel(9, 2));
        assert!(!joins.cancel(1, 9));
        assert!(joins.cancel(1, 2));
        tokio::time::timeout(Duration::from_millis(50), pending.cancelled())
            .await
            .unwrap();
        assert!(!pending.commit(|| panic!("cancelled join must not publish a player")));
    }

    #[test]
    fn old_join_cleanup_cannot_clear_a_new_reservation() {
        let joins = Arc::new(JoinCancellation::default());
        let old = joins.begin(1, 2);
        let new = joins.begin(1, 2);
        drop(old);
        assert!(joins.cancel(1, 2));
        assert!(!new.commit(|| panic!("cancelled new join")));
    }

    #[test]
    fn published_join_is_no_longer_a_pending_cancellation_target() {
        let joins = Arc::new(JoinCancellation::default());
        let join = joins.begin(1, 2);
        let mut published = false;
        assert!(join.commit(|| published = true));
        assert!(published);
        assert!(!joins.cancel(1, 2));
    }

    #[tokio::test]
    async fn fast_reply_sends_one_private_callback_with_no_mentions() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let (end, length) = loop {
                let mut block = [0; 4096];
                let count = stream.read(&mut block).await.unwrap();
                assert!(count > 0 && data.len() + count <= 65536);
                data.extend_from_slice(&block[..count]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if data.len() >= end + 4 + length {
                        break (end, length);
                    }
                }
            };
            assert!(String::from_utf8_lossy(&data[..end]).starts_with("POST "));
            let body: serde_json::Value =
                serde_json::from_slice(&data[end + 4..end + 4 + length]).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            body
        });
        let command: CommandInteraction = serde_json::from_value(serde_json::json!({
            "id":"1", "application_id":"2", "type":2,
            "data":{"id":"5", "name":"ping", "type":1},
            "channel_id":"4", "token":"fixture-token", "version":1,
            "user":{"id":"3", "username":"fixture", "discriminator":"0", "avatar":null},
            "locale":"en-US", "entitlements":[], "attachment_size_limit":8388608
        }))
        .unwrap();
        let http = serenity::http::HttpBuilder::new("fixture-not-a-real-token")
            .proxy(format!("http://{address}"))
            .ratelimiter_disabled(true)
            .build();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(2),
                send_reply(&command, &http, "Application: Pong.".into(), vec![], false)
            )
            .await
            .unwrap()
        );
        let body = server.await.unwrap();
        assert_eq!(body["type"], 4); // A final callback, not a deferred loading state.
        assert_eq!(body["data"]["flags"], 64); // Preserve ephemeral privacy.
        assert_eq!(body["data"]["content"], "Application: Pong.");
        assert_eq!(
            body["data"]["allowed_mentions"]["parse"],
            serde_json::json!([])
        );
    }
}
