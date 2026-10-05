# SolDiscordBot

A multi-purpose Discord bot suite, beginning with an advanced music module designed for an Orange Pi Zero 3 running DietPi and Docker Compose.

## Project status

Requirements and architecture proposal prepared; stack confirmation pending. This repository does not yet contain a runnable bot.

The development brief requires research and stack confirmation before implementation. See [the original project brief](docs/project-brief.md) and [the roadmap](docs/roadmap.md).

## Deployment target

- Orange Pi Zero 3: ARM64 Cortex-A53, 4 GB RAM.
- DietPi on microSD storage.
- Docker Compose with `restart: unless-stopped`.
- RAM-backed temporary audio buffers, transcode caches, and frequent logs to reduce microSD writes.

## Music module goals

- Application slash commands and interactive buttons, modals, and select menus.
- Asynchronous search, resolution, queueing, and playback.
- Guild-specific nickname with a fallback to the global application name.
- Search ranking that favors official and studio releases unless the user requests another version.
- Playback and queue controls, saved playlists, seeking, repeat modes, volume, filters, lyrics, history, and server settings.
- True overlapping audio crossfades with configurable transitions from 3 to 10 seconds.
- Provider coverage goals: YouTube, YouTube Music, SoundCloud, Deezer, Apple Music, Bandcamp, and Spotify.

Provider playback capabilities, metadata-only integrations, and resource limits must be verified during research; these goals are not claims of implemented support.

## Next milestone

Confirm the proposed **Rust + Serenity + Songbird** stack, then implement guild identity, intelligent /play and actual overlapping /crossfade. Performance must be measured on the target board before increasing concurrency.

Review the [stack proposal](docs/stack-proposal.md), [architecture](docs/architecture.md), [audio engine design](docs/audio-engine.md), [provider capabilities](docs/providers.md), and [Docker deployment outline](docs/deployment.md). The [docker-compose.yml](docker-compose.yml) is a guarded design outline with a placeholder image, not a working deployment.

## Secrets

Keep Discord tokens and provider credentials outside version control. Use environment variables or deployment secrets; only placeholder configuration examples should be committed.
