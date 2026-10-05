# SolDiscordBot

Rust + Serenity 0.12.5 + Songbird 0.6.0 music foundation for an Orange Pi Zero 3 running ARM64 DietPi. The owner confirmed this stack on 5 October 2026.

Implemented: `/play`, `/join`, `/disconnect`, `/pause`, `/skip`, `/stop`, `/queue`, `/nowplaying`, `/crossfade`, `/ping`, `/help`, playback buttons and expiring release-selection menus. Guild help uses the bot nickname, falling back to the Developer Portal application name. YouTube (including Music URLs), SoundCloud and Bandcamp public tracks/playlists use bounded yt-dlp extraction; other providers return capability errors.

`/play position:Next` preserves playlist order ahead of queued tracks. `position:Now` interrupts both active sources. Search ranking penalizes unintended covers/live/remixes and asks you to select when confidence or separation is low. Metadata hints and verified uploaders do not establish release ownership.

Crossfade uses Songbird's additive mixer with two concurrent FFmpeg sources. Linear/equal-power envelopes run at a 20 ms control cadence from consumed media time; pausing freezes both sources. Known-duration tracks overlap for 3–10 seconds when preparation finishes in time. Late, live, or unknown-duration tracks advance normally. This is implemented overlap, but recorded voice continuity and audible-click testing remain deployment gates.

## Run on ARM64

1. Install Docker and Compose on DietPi. Clone this repository and check out the PR branch during development.
2. Copy `.env.example` to `.env`; set `DISCORD_TOKEN` and `DISCORD_TEST_GUILD_ID`. Invite the application with `bot` and `applications.commands` scopes, with View Channel, Connect and Speak permissions. Guilds and Guild Voice States are the only gateway intents; no privileged intents are required.
3. Run `docker compose config`, then `docker compose up --build -d`. The first native ARM64 build may take substantial time; CI also checks an ARM64 container build.
4. Use `/join`, `/play`, then `/crossfade enabled:true seconds:5 curve:equal_power` with at least two queued finite tracks.

The container is non-root, read-only, capped at one connected guild, two decoder processes, one extractor, eight pending searches and 200 queued tracks. All runtime writes use bounded tmpfs mounts; Docker logging is disabled. Queue/crossfade settings are RAM-only and reset on restart. No token or credentials are included.

For local Linux development, install Rust 1.99.0, CMake, pkg-config, libopus development headers, FFmpeg, Python and Node 24; install `yt-dlp[default]==2026.8.19` into an isolated environment. Export the two Discord variables, then `cargo run --locked`. Without Compose, logs and heartbeat default to the OS temp directory: configure RAM paths explicitly for deployment.

## Verify

```sh
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

CI executes these checks and builds/runs the ARM64 image's dependency tools. Tests cover version ranking, Unicode queries, ambiguity, playlist insertion, queue limits, simultaneous signal gains, stale playback requests, preparation cancellation and decoder-process cleanup. They do not replace a live Discord voice test or an Orange Pi benchmark.

See [deployment](docs/deployment.md), [validation](docs/validation.md), [architecture](docs/architecture.md), [provider capabilities](docs/providers.md), [roadmap](docs/roadmap.md) and the [original brief](docs/project-brief.md). Saved playlists, SQLite settings, seek, filters, lyrics, history, loop/shuffle, DJ roles, announcements and additional provider adapters remain future milestones.
