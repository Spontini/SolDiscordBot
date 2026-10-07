use crate::{
    model::{Curve, Media, Position},
    player::Player,
    resolver::{Resolution, Resolver},
    responsiveness::{IdentityCache, JoinCancellation, initial_result, send_reply},
};
use anyhow::{Context as _, Result, bail};
use serenity::{all::*, async_trait};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Semaphore};

struct Active {
    guild: GuildId,
    channel: ChannelId,
    player: Player,
}
struct Choice {
    guild: GuildId,
    user: UserId,
    player: Player,
    epoch: u64,
    position: Position,
    tracks: Vec<Media>,
    expires: Instant,
}

pub struct App {
    pub application_name: String,
    pub test_guild: Option<GuildId>,
    pub healthy: Arc<AtomicBool>,
    registered: AtomicBool,
    active: Mutex<Option<Active>>,
    voice_operations: Mutex<()>,
    joins: Arc<JoinCancellation>,
    resolver: Resolver,
    decoders: Arc<Semaphore>,
    choices: Mutex<HashMap<String, Choice>>,
    identities: Arc<IdentityCache>,
}
impl App {
    pub fn new(
        application_name: String,
        test_guild: Option<GuildId>,
        healthy: Arc<AtomicBool>,
    ) -> Self {
        Self {
            application_name,
            test_guild,
            healthy,
            registered: AtomicBool::new(false),
            active: Mutex::new(None),
            voice_operations: Mutex::new(()),
            joins: Arc::new(JoinCancellation::default()),
            resolver: Resolver::default(),
            decoders: Arc::new(Semaphore::new(2)),
            choices: Mutex::new(HashMap::new()),
            identities: Arc::new(IdentityCache::default()),
        }
    }
    pub async fn shutdown(&self) {
        let active = self.active.lock().await.take();
        if let Some(active) = active {
            active.player.stop(true).await;
        }
    }
    fn identity(&self, ctx: &Context, guild: GuildId) -> String {
        let bot_id = ctx.cache.current_user().id;
        let cached_nick = ctx
            .cache
            .guild(guild)
            .and_then(|g| g.members.get(&bot_id).and_then(|m| m.nick.clone()));
        let fallback = crate::model::identity_name(cached_nick.as_deref(), &self.application_name);
        let (name, refresh) = self
            .identities
            .lookup(guild.get(), fallback, Instant::now());
        if let Some(revision) = refresh {
            let cache = self.identities.clone();
            let http = ctx.http.clone();
            let application = self.application_name.clone();
            tokio::spawn(async move {
                if let Ok(Ok(member)) =
                    tokio::time::timeout(Duration::from_secs(2), http.get_member(guild, bot_id))
                        .await
                {
                    cache.finish(
                        guild.get(),
                        revision,
                        crate::model::identity_name(member.nick.as_deref(), &application),
                    );
                }
            });
        }
        name
    }
    fn voice_channel(ctx: &Context, guild: GuildId, user: UserId) -> Result<ChannelId> {
        ctx.cache
            .guild(guild)
            .and_then(|g| g.voice_states.get(&user).and_then(|v| v.channel_id))
            .context("Join a voice channel first.")
    }
    async fn player(
        &self,
        ctx: &Context,
        guild: GuildId,
        user: UserId,
        join: bool,
    ) -> Result<Player> {
        let channel = Self::voice_channel(ctx, guild, user)?;
        let active = self.active.lock().await;
        if let Some(current) = active.as_ref() {
            if current.guild != guild {
                bail!("The single-guild player limit is in use. Disconnect the other guild first.");
            }
            if current.channel != channel {
                bail!("Join the bot's voice channel to control playback.");
            }
            return Ok(current.player.clone());
        }
        if !join {
            bail!("The bot is not connected. Use /join or /play.");
        }
        drop(active);
        // Serialize voice transitions without blocking state readers or events.
        // A concurrent transition gets a prompt answer instead of waiting 20s.
        let _operation = self
            .voice_operations
            .try_lock()
            .context("A voice connection change is already in progress. Try again shortly.")?;
        let active = self.active.lock().await;
        if let Some(current) = active.as_ref() {
            if current.guild != guild || current.channel != channel {
                bail!("The voice session changed. Try the command again.");
            }
            return Ok(current.player.clone());
        }
        drop(active);
        let manager = songbird::get(ctx)
            .await
            .context("Voice manager is unavailable.")?;
        let joined = Instant::now();
        let mut reservation = self.joins.begin(guild.get(), channel.get());
        let result = tokio::select! {
            result=tokio::time::timeout(Duration::from_secs(20), manager.join(guild, channel))=>result,
            _=reservation.cancelled()=>{
                let _=manager.remove(guild).await;
                bail!("Voice connection cancelled.");
            }
        };
        let call = match result {
            Ok(Ok(call)) => call,
            failed => {
                let failure = if failed.is_err() {
                    "timeout"
                } else {
                    "join_error"
                };
                tracing::warn!(
                    failure,
                    elapsed_ms = joined.elapsed().as_millis() as u64,
                    "voice_join_failed"
                );
                let _ = manager.remove(guild).await;
                bail!(
                    "Voice connection failed or timed out. Check Connect/Speak permissions and try again."
                );
            }
        };
        tracing::info!(
            elapsed_ms = joined.elapsed().as_millis() as u64,
            "voice_join_complete"
        );
        let setup = {
            let mut call = call.lock().await;
            let result = call.deafen(true).await;
            call.set_bitrate(songbird::driver::Bitrate::Bits(96_000));
            result
        };
        if setup.is_err() {
            let _ = manager.remove(guild).await;
            bail!("Could not configure the voice connection.");
        }
        let player = Player::new(call, self.resolver.clone(), self.decoders.clone());
        let mut active = self.active.lock().await;
        let accepted = reservation.commit(|| {
            *active = Some(Active {
                guild,
                channel,
                player: player.clone(),
            })
        });
        drop(active);
        if !accepted {
            player.stop(true).await;
            let _ = manager.remove(guild).await;
            bail!("Voice connection cancelled.");
        }
        Ok(player)
    }
    async fn command(
        &self,
        ctx: &Context,
        command: &CommandInteraction,
    ) -> Result<(String, Vec<CreateActionRow>)> {
        let guild = command
            .guild_id
            .context("Use these commands in a server.")?;
        let user = command.user.id;
        let empty = Vec::new();
        let content = match command.data.name.as_str() {
            "play" => {
                let query =
                    string_option(command, "query").context("Provide a query or provider URL.")?;
                let position = match string_option(command, "position").unwrap_or("End") {
                    "Next" => Position::Next,
                    "Now" => Position::Now,
                    _ => Position::End,
                };
                let player = self.player(ctx, guild, user, true).await?;
                let epoch = player.epoch();
                let resolving = Instant::now();
                let resolution = tokio::select! {
                    resolution=self.resolver.resolve(query)=>resolution?,
                    _=async{loop{tokio::time::sleep(Duration::from_millis(100)).await;if player.epoch()!=epoch{break;}}}=>bail!("Playback request cancelled.")
                };
                tracing::info!(elapsed_ms = resolving.elapsed().as_millis() as u64, "play_resolution_complete");
                // Recheck voice membership after slow provider I/O.
                self.player(ctx, guild, user, false).await?;
                match resolution {
                    Resolution::Batch(batch) => format!(
                        "Queued {} track(s).",
                        player.enqueue(batch, position, epoch).await?
                    ),
                    Resolution::Choices(tracks) => {
                        let id = format!("pick:{}", command.id.get());
                        let options = tracks
                            .iter()
                            .enumerate()
                            .map(|(i, m)| {
                                CreateSelectMenuOption::new(short(&m.title, 95), i.to_string())
                                    .description(short(&m.artist, 95))
                            })
                            .collect();
                        let mut choices = self.choices.lock().await;
                        choices.retain(|_, c| c.expires > Instant::now());
                        if choices.len() >= 64 {
                            bail!("Too many pending selections. Try again shortly.");
                        }
                        choices.insert(
                            id.clone(),
                            Choice {
                                guild,
                                user,
                                player,
                                epoch,
                                position,
                                tracks,
                                expires: Instant::now() + Duration::from_secs(120),
                            },
                        );
                        return Ok((
                            "Several releases match. Choose one within two minutes.".into(),
                            vec![CreateActionRow::SelectMenu(CreateSelectMenu::new(
                                id,
                                CreateSelectMenuKind::String { options },
                            ))],
                        ));
                    }
                }
            }
            "join" => {
                self.player(ctx, guild, user, true).await?;
                "Joined your voice channel.".into()
            }
            "pause" => self.player(ctx, guild, user, false).await?.pause().await?,
            "skip" => self.player(ctx, guild, user, false).await?.skip().await,
            "stop" => {
                self.player(ctx, guild, user, false)
                    .await?
                    .stop(false)
                    .await
            }
            "disconnect" => {
                let channel = Self::voice_channel(ctx, guild, user)?;
                if self.joins.cancel(guild.get(), channel.get()) {
                    return Ok(("Cancelled the pending voice connection.".into(), empty));
                }
                let player = self.player(ctx, guild, user, false).await?;
                let _operation = self.voice_operations.try_lock()
                    .context("A voice connection change is already in progress. Try again shortly.")?;
                let active = self.active.lock().await;
                if !active.as_ref().is_some_and(|current|current.guild==guild && current.player.same_session(&player)) {
                    bail!("The voice session changed. Try the command again.");
                }
                drop(active);
                let leaving = Instant::now();
                player.stop(true).await;
                if let Some(manager) = songbird::get(ctx).await {
                    manager.remove(guild).await?;
                }
                let mut active = self.active.lock().await;
                if active.as_ref().is_some_and(|current| current.guild == guild && current.player.same_session(&player)) {
                    *active = None;
                }
                drop(active);
                tracing::info!(elapsed_ms=leaving.elapsed().as_millis() as u64, "voice_disconnect_complete");
                "Disconnected.".into()
            }
            "queue" | "nowplaying" => {
                let player = self.player(ctx, guild, user, false).await?;
                let text = player.describe(command.data.name == "queue").await;
                return Ok((
                    text,
                    vec![CreateActionRow::Buttons(vec![
                        CreateButton::new("control:pause").label("Pause / resume"),
                        CreateButton::new("control:skip").label("Skip"),
                        CreateButton::new("control:stop").label("Stop"),
                    ])],
                ));
            }
            "crossfade" => {
                let enabled = command
                    .data
                    .options
                    .iter()
                    .find(|o| o.name == "enabled")
                    .and_then(|o| o.value.as_bool())
                    .context("Choose enabled or disabled.")?;
                let seconds = command
                    .data
                    .options
                    .iter()
                    .find(|o| o.name == "seconds")
                    .and_then(|o| o.value.as_i64())
                    .unwrap_or(5) as f64;
                let curve = if string_option(command, "curve") == Some("equal_power") {
                    Curve::EqualPower
                } else {
                    Curve::Linear
                };
                self.player(ctx, guild, user, false)
                    .await?
                    .crossfade(enabled, seconds, curve)
                    .await?
            }
            "ping" => "Pong.".into(),
            "help" => "/play, /join, /pause, /skip, /stop, /disconnect, /queue, /nowplaying, /crossfade, /ping. Playback: YouTube (including Music URLs), SoundCloud and Bandcamp public media. Crossfade uses two simultaneous tracks; enable it with /crossfade. Saved playlists, seeking and additional providers are planned.".into(),
            _ => bail!("Unknown command."),
        };
        Ok((content, empty))
    }
    async fn component(&self, ctx: &Context, interaction: &ComponentInteraction) -> Result<String> {
        let guild = interaction
            .guild_id
            .context("Use this control in a server.")?;
        if interaction.data.custom_id.starts_with("control:") {
            let player = self.player(ctx, guild, interaction.user.id, false).await?;
            return match interaction.data.custom_id.as_str() {
                "control:pause" => player.pause().await,
                "control:skip" => Ok(player.skip().await),
                "control:stop" => Ok(player.stop(false).await),
                _ => bail!("Unknown control."),
            };
        }
        let choice = {
            let mut choices = self.choices.lock().await;
            let choice = choices
                .get(&interaction.data.custom_id)
                .context("Selection expired or already used.")?;
            if choice.user != interaction.user.id || choice.guild != guild {
                bail!("This selection belongs to another user.");
            }
            choices.remove(&interaction.data.custom_id).unwrap()
        };
        if choice.expires <= Instant::now() {
            bail!("Selection expired. Run /play again.");
        }
        let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind else {
            bail!("Invalid selection.");
        };
        let index = values
            .first()
            .context("Choose a release.")?
            .parse::<usize>()?;
        let media = choice
            .tracks
            .get(index)
            .context("Invalid release selection.")?
            .clone();
        self.player(ctx, guild, interaction.user.id, false).await?;
        choice
            .player
            .enqueue(vec![media], choice.position, choice.epoch)
            .await?;
        Ok("Queued the selected release.".into())
    }
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
fn string_option<'a>(command: &'a CommandInteraction, name: &str) -> Option<&'a str> {
    command
        .data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.value.as_str())
}
fn commands() -> Vec<CreateCommand> {
    let mut commands = [
        ("join", "Join your voice channel"),
        ("disconnect", "Leave voice and clear playback"),
        ("pause", "Pause or resume both active tracks"),
        ("skip", "Skip the current track"),
        ("stop", "Stop playback and clear the queue"),
        ("queue", "Show the next ten queued tracks"),
        ("nowplaying", "Show current playback and controls"),
        ("ping", "Check command responsiveness"),
        ("help", "Show commands and provider capabilities"),
    ]
    .into_iter()
    .map(|(n, d)| CreateCommand::new(n).description(d).dm_permission(false))
    .collect::<Vec<_>>();
    commands.push(
        CreateCommand::new("play")
            .description("Play a query or public provider URL")
            .dm_permission(false)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "query",
                    "Artist/title, track URL or playlist URL",
                )
                .required(true)
                .max_length(500),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "position",
                    "Queue insertion position",
                )
                .add_string_choice("End", "End")
                .add_string_choice("Next", "Next")
                .add_string_choice("Now", "Now"),
            ),
    );
    commands.push(
        CreateCommand::new("crossfade")
            .description("Configure real overlapping transitions for known-duration tracks")
            .dm_permission(false)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Enable overlapping transitions",
                )
                .required(true),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::Integer,
                    "seconds",
                    "Duration from 3 to 10 seconds",
                )
                .min_int_value(3)
                .max_int_value(10),
            )
            .add_option(
                CreateCommandOption::new(CommandOptionType::String, "curve", "Transition envelope")
                    .add_string_choice("Linear", "linear")
                    .add_string_choice("Equal power", "equal_power"),
            ),
    );
    commands
}

#[async_trait]
impl EventHandler for App {
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: Option<bool>) {
        // Prewarm identity independently of the first interaction.
        self.identity(&ctx, guild.id);
    }
    async fn guild_member_update(
        &self,
        ctx: Context,
        _old: Option<Member>,
        _new: Option<Member>,
        event: GuildMemberUpdateEvent,
    ) {
        if event.user.id == ctx.cache.current_user().id {
            self.identities.update(
                event.guild_id.get(),
                crate::model::identity_name(event.nick.as_deref(), &self.application_name),
            );
        }
    }
    async fn ready(&self, ctx: Context, _ready: Ready) {
        if !self.registered.swap(true, Ordering::SeqCst) {
            let result = if let Some(guild) = self.test_guild {
                guild.set_commands(&ctx.http, commands()).await
            } else {
                Command::set_global_commands(&ctx.http, commands()).await
            };
            if result.is_err() {
                self.registered.store(false, Ordering::SeqCst);
                tracing::error!("command registration failed");
                return;
            }
        }
        self.healthy.store(true, Ordering::SeqCst);
        tracing::info!("Discord gateway ready");
    }
    async fn shard_stage_update(
        &self,
        _ctx: Context,
        event: serenity::gateway::ShardStageUpdateEvent,
    ) {
        if event.new != serenity::gateway::ConnectionStage::Connected {
            self.healthy.store(false, Ordering::SeqCst);
        }
    }
    async fn resume(&self, _ctx: Context, _event: ResumedEvent) {
        if self.registered.load(Ordering::SeqCst) {
            self.healthy.store(true, Ordering::SeqCst);
        }
    }
    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        if new.user_id != ctx.cache.current_user().id {
            return;
        }
        let mut active = self.active.lock().await;
        if let Some(current) = active.as_mut() {
            if new.guild_id == Some(current.guild) {
                if let Some(channel) = new.channel_id {
                    current.channel = channel;
                } else {
                    let current = active.take().expect("active voice session");
                    drop(active);
                    current.player.stop(true).await;
                }
            }
        }
    }
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(command) => {
                let started = Instant::now();
                let kind = command_kind(&command.data.name);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let created = u128::from((command.id.get() >> 22) + 1_420_070_400_000);
                tracing::info!(
                    command = kind,
                    gateway_delay_ms = now.saturating_sub(created) as u64,
                    "command_received"
                );
                let name = command.guild_id.map(|guild| self.identity(&ctx, guild));
                let work = self.command(&ctx, &command);
                tokio::pin!(work);
                let immediate = if matches!(kind, "play" | "join") {
                    None
                } else {
                    initial_result(work.as_mut()).await
                };
                let deferred = immediate.is_none();
                let result = if let Some(result) = immediate {
                    result
                } else {
                    let request = Instant::now();
                    let ok = command.defer_ephemeral(&ctx.http).await.is_ok();
                    tracing::info!(
                        command = kind,
                        deferred = true,
                        ok,
                        request_ms = request.elapsed().as_millis() as u64,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "command_initial_response"
                    );
                    if !ok {
                        // A quick control may already have started before its
                        // 20ms budget elapsed. Finish that authorized operation
                        // once, rather than abandoning a half-finished leave.
                        if !matches!(kind, "play" | "join") {
                            let finished = tokio::time::timeout(Duration::from_secs(5), work)
                                .await
                                .is_ok();
                            tracing::warn!(
                                command = kind,
                                finished,
                                "command_ack_failed_after_work_started"
                            );
                        }
                        return;
                    }
                    work.await
                };
                tracing::info!(
                    command = kind,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "command_work_complete"
                );
                let (content, components) =
                    result.unwrap_or_else(|error| (format!("{error}"), vec![]));
                let content = match name {
                    Some(name) => format!("{name}: {content}"),
                    None => content,
                };
                let request = Instant::now();
                let ok = send_reply(
                    &command,
                    &ctx.http,
                    short(&content, 1900),
                    components,
                    deferred,
                )
                .await;
                tracing::info!(
                    command = kind,
                    deferred,
                    ok,
                    request_ms = request.elapsed().as_millis() as u64,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "command_complete"
                );
                if !ok {
                    tracing::warn!("command response failed");
                }
            }
            Interaction::Component(component) => {
                if component.defer_ephemeral(&ctx.http).await.is_err() {
                    return;
                }
                let content = self
                    .component(&ctx, &component)
                    .await
                    .unwrap_or_else(|error| error.to_string());
                if component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(short(&content, 1900))
                            .allowed_mentions(CreateAllowedMentions::new()),
                    )
                    .await
                    .is_err()
                {
                    tracing::warn!("component response failed");
                }
            }
            _ => {}
        }
    }
}

fn command_kind(name: &str) -> &'static str {
    match name {
        "play" => "play",
        "join" => "join",
        "disconnect" => "disconnect",
        "pause" => "pause",
        "skip" => "skip",
        "stop" => "stop",
        "queue" => "queue",
        "nowplaying" => "nowplaying",
        "crossfade" => "crossfade",
        "ping" => "ping",
        "help" => "help",
        _ => "unknown",
    }
}
