use crate::{
    model::{Curve, Media, Position},
    player::Player,
    resolver::{Resolution, Resolver},
};
use anyhow::{Context as _, Result, bail};
use serenity::{all::*, async_trait};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
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
type IdentityCache = HashMap<GuildId, (Instant, String)>;

pub struct App {
    pub application_name: String,
    pub test_guild: Option<GuildId>,
    pub healthy: Arc<AtomicBool>,
    registered: AtomicBool,
    active: Mutex<Option<Active>>,
    resolver: Resolver,
    decoders: Arc<Semaphore>,
    choices: Mutex<HashMap<String, Choice>>,
    identities: Mutex<IdentityCache>,
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
            resolver: Resolver::default(),
            decoders: Arc::new(Semaphore::new(2)),
            choices: Mutex::new(HashMap::new()),
            identities: Mutex::new(HashMap::new()),
        }
    }
    pub async fn shutdown(&self) {
        if let Some(active) = self.active.lock().await.take() {
            active.player.stop(true).await;
        }
    }
    async fn identity(&self, ctx: &Context, guild: GuildId) -> String {
        {
            let cache = self.identities.lock().await;
            if let Some((time, name)) = cache.get(&guild) {
                if time.elapsed() < Duration::from_secs(300) {
                    return name.clone();
                }
            }
        }
        let nick = ctx
            .http
            .get_member(guild, ctx.cache.current_user().id)
            .await
            .ok()
            .and_then(|member| member.nick)
            .filter(|n| !n.trim().is_empty());
        let name = nick.unwrap_or_else(|| self.application_name.clone());
        let mut cache = self.identities.lock().await;
        cache.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(300));
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(guild, (Instant::now(), name.clone()));
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
        let mut active = self.active.lock().await;
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
        let manager = songbird::get(ctx)
            .await
            .context("Voice manager is unavailable.")?;
        let call = tokio::time::timeout(Duration::from_secs(20), manager.join(guild, channel))
            .await
            .context("Voice connection timed out.")??;
        {
            let mut call = call.lock().await;
            call.deafen(true).await?;
            call.set_bitrate(songbird::Bitrate::Bits(96_000));
        }
        let player = Player::new(call, self.resolver.clone(), self.decoders.clone());
        *active = Some(Active {
            guild,
            channel,
            player: player.clone(),
        });
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
                let resolution = tokio::select! {
                    resolution=self.resolver.resolve(query)=>resolution?,
                    _=async{loop{tokio::time::sleep(Duration::from_millis(100)).await;if player.epoch()!=epoch{break;}}}=>bail!("Playback request cancelled.")
                };
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
                let player = self.player(ctx, guild, user, false).await?;
                let mut active = self.active.lock().await;
                player.stop(true).await;
                if let Some(manager) = songbird::get(ctx).await {
                    manager.remove(guild).await?;
                }
                *active = None;
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
            "ping" => "Command handler is online.".into(),
            "help" => format!(
                "{}: /play, /join, /pause, /skip, /stop, /disconnect, /queue, /nowplaying, /crossfade, /ping. Playback: YouTube (including Music URLs), SoundCloud and Bandcamp public media. Crossfade uses two simultaneous tracks; enable it with /crossfade. Saved playlists, seeking and additional providers are planned.",
                self.identity(ctx, guild).await
            ),
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
                    current.player.stop(true).await;
                    *active = None;
                }
            }
        }
    }
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(command) => {
                if command.defer_ephemeral(&ctx.http).await.is_err() {
                    return;
                }
                let (content, components) = self
                    .command(&ctx, &command)
                    .await
                    .unwrap_or_else(|error| (format!("{error}"), vec![]));
                let response = EditInteractionResponse::new()
                    .content(short(&content, 1900))
                    .components(components)
                    .allowed_mentions(CreateAllowedMentions::new());
                if command.edit_response(&ctx.http, response).await.is_err() {
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
