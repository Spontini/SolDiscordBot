# Stack proposal

Status: owner confirmed Rust + Serenity + Songbird on 5 October 2026; foundational implementation is under development. Research date: 5 October 2026.

## Recommendation

Use **Rust + Serenity + Songbird**, with Tokio for asynchronous orchestration, native libopus for voice encoding, FFmpeg for formats/effects that need it, and SQLite for saved playlists and guild settings. Use yt-dlp as a bounded subprocess for supported media extraction, with its EJS component and a supported JavaScript runtime packaged in the image.

This recommendation prioritizes the specific target: a 4 GB ARM64 board, two overlapping music sources, and a future multi-purpose bot. Rust is a fit for predictable resource use and an existing native audio mixer. Actual throughput remains a hardware measurement, not a language guarantee.

## Options considered

| Option | Strength | Cost for this project |
| --- | --- | --- |
| Rust / Serenity / Songbird | Native multi-track mixer, asynchronous Discord wrapper, DAVE support | More involved build and ownership model; transition scheduling still needs application code |
| TypeScript / discord.js / @discordjs/voice | Extensive Discord UI tooling and maintained DAVE transport | Requires a separate real-time mixer/controller for overlapping sources |
| Python / discord.py 2.7+ | Async APIs and current DAVE support | Custom mixing and careful separation of CPU work from the event loop |
| Bot client + Lavalink | Established remote audio-node model | Its standard player API exposes one current track and no two-track crossfade control; a separate plugin/engine would need validation |

These are viable alternatives, not claims that a language or library is universally best. [discord.js](https://discord.js.org/docs/packages/discord.js/stable), [voice](https://discord.js.org/docs/packages/voice/stable), [discord.py changelog](https://discordpy.readthedocs.io/en/stable/whats_new.html), [Lavalink player API](https://lavalink.dev/api/rest#player-api).

## Core dependencies

| Component | Proposed selection | Implementation rule |
| --- | --- | --- |
| Discord wrapper | Serenity 0.12.5 stable | Direct application commands and component handlers; disable the deprecated standard framework |
| Audio engine | Songbird 0.6.0 stable | Enable driver, Serenity integration, DAVE and appropriate TLS features; disable voice receive |
| Async runtime | Tokio stable, compatible with the selected crates | Bounded channels/semaphores; no blocking process reads on async workers |
| HTTP | Compatible maintained reqwest with rustls | Shared client, timeouts, bounded responses and cancellation |
| Media decoding | Songbird/Symphonia supported formats; FFmpeg when needed | Enable needed codecs explicitly; never assume defaults decode every format |
| Encoding | Songbird's opus2 / native libopus path | One encoder per guild output, including during crossfades |
| Metadata/extraction | yt-dlp + matching EJS package + supported JS runtime | Versions installed and pinned at build time; bounded subprocesses |
| Persistence | SQLite through a maintained Rust driver | Low-frequency durable user edits; transient state stays in RAM |

The currently inspected stable releases are [Serenity 0.12.5](https://docs.rs/serenity/latest/serenity/) and [Songbird 0.6.0](https://github.com/serenity-rs/songbird/blob/current/CHANGELOG.md). Songbird 0.6 added DAVE and switched to opus2; its minimum Rust version is 1.83. Recheck published releases when implementation begins, commit Cargo.lock, and pin build/runtime images by digest after a successful ARM64 build. Avoid copying the older 0.5 dependency example from Songbird's README.

Discord requires DAVE-capable clients for ordinary voice calls. Voice connection tests must verify this, including reconnects. [Discord migration announcement](https://discord.com/blog/bringing-dave-to-all-discord-platforms).

## Deployment decision

One bot container contains the application and in-process Songbird driver. No JVM audio node, Redis, or database service is needed for the first phase. Startup checks validate native libraries, media tools, writable RAM mounts, and durable storage before connecting to Discord. SQLite requires no Compose dependency service.

Use a Debian/glibc runtime on linux/arm64 with native libopus, FFmpeg, Python for yt-dlp, and a supported JS runtime for EJS. Build on an ARM64 CI runner or an external builder, rather than repeatedly compiling on the microSD card. EJS's guide recommends Deno and also supports Node; the implementation should pin a verified ARM64 runtime and matching yt-dlp/EJS versions. [Songbird codec/dependency documentation](https://github.com/serenity-rs/songbird#codec-support), [yt-dlp EJS setup](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

## What confirmation authorizes

Confirming this proposal starts foundational implementation of guild identity, intelligent /play, queue state, genuine two-track /crossfade, deployment image, and meaningful audio/queue tests. The remaining commands follow the staged roadmap. No performance or provider feature is considered complete until its validation gate passes.

See [architecture](architecture.md), [audio design](audio-engine.md), [provider capability plan](providers.md), and [deployment outline](deployment.md).
