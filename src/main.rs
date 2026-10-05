use anyhow::{Context, Result, bail};
use serenity::{
    Client,
    all::{GatewayIntents, GuildId},
};
use soldiscordbot::discord::App;
use songbird::SerenityInit;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn health_path() -> PathBuf {
    std::env::var_os("SOL_HEALTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("sol-health"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn check_health() -> Result<()> {
    let value = std::fs::read_to_string(health_path())?.parse::<u64>()?;
    if now().saturating_sub(value) > 90 {
        bail!("Gateway readiness heartbeat is stale.");
    }
    Ok(())
}
async fn verify_tools() -> Result<()> {
    for (tool, arg) in [
        ("ffmpeg", "-version"),
        ("yt-dlp", "--version"),
        ("node", "--version"),
    ] {
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(tool)
                .arg(arg)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await
        .context("Dependency check timed out.")??;
        if !status.success() {
            bail!("Required dependency {tool} is unavailable.");
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|arg| arg == "--healthcheck") {
        return check_health();
    }
    let log_dir = std::env::var_os("SOL_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("sol-logs"));
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("bot")
        .max_log_files(4)
        .build(log_dir)?;
    let (writer, _guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(1024)
        .lossy(true)
        .finish(appender);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter("soldiscordbot=info,serenity=error,songbird=error")
        .with_writer(writer)
        .init();
    verify_tools().await?;
    let token = std::env::var("DISCORD_TOKEN").context("Set DISCORD_TOKEN.")?;
    let http = serenity::http::Http::new(&token);
    // This is the Developer Portal application name, never the bot username.
    let application = http.get_current_application_info().await?;
    let test_guild = std::env::var("DISCORD_TEST_GUILD_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().map(GuildId::new))
        .transpose()
        .context("Invalid DISCORD_TEST_GUILD_ID.")?;
    let healthy = Arc::new(AtomicBool::new(false));
    let app = Arc::new(App::new(application.name, test_guild, healthy.clone()));
    let mut client = Client::builder(
        &token,
        GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES,
    )
    .event_handler_arc(app.clone())
    .register_songbird_from_config(songbird::Config::default().preallocated_tracks(2))
    .await?;
    let heartbeat = tokio::spawn(async move {
        loop {
            if healthy.load(Ordering::SeqCst) {
                let path = health_path();
                let temp = path.with_extension("new");
                if tokio::fs::write(&temp, now().to_string()).await.is_ok() {
                    let _ = tokio::fs::rename(temp, path).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
    let result = tokio::select! {result=client.start()=>result.map_err(anyhow::Error::from),_=shutdown_signal()=>Ok(())};
    app.shutdown().await;
    client.shard_manager.shutdown_all().await;
    heartbeat.abort();
    let _ = tokio::fs::remove_file(health_path()).await;
    result
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
