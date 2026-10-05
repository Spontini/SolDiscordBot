# ARM64 deployment

The Dockerfile builds Rust 1.99.0 with Cargo.lock and provides FFmpeg, native Opus, Node 24 and `yt-dlp[default]==2026.8.19` (including EJS). CI builds natively on an ARM64 Linux runner; Orange Pi performance still needs measurement.

Copy `.env.example` to `.env`, supply the token/test guild, run `docker compose config`, then `docker compose up --build -d`. No incoming ports or external audio/database services are needed. The bot checks FFmpeg, yt-dlp and Node before opening the gateway. Command registration uses DISCORD_TEST_GUILD_ID when supplied; otherwise it registers global commands.

The runtime uses UID/GID 10001, a read-only image, dropped capabilities and no-new-privileges. Compose caps memory at 1536 MiB, equal total memory/swap, CPU at three cores and processes at 128. These values require target measurement before increasing the one-guild limit. Durable storage is not mounted because saved playlists/settings are not implemented yet.

| Runtime writes | RAM location | Capacity |
| --- | --- | --- |
| Temporary files/extractor cache | /tmp | 96 MiB tmpfs |
| Application logs | /var/log/sol | 16 MiB tmpfs |
| Readiness heartbeat | /run/sol | 8 MiB tmpfs |
| Audio | Native buffers and process pipes | Two sources, no whole-file download |

Logs use a non-blocking writer, daily files with at most four retained files, and the 16 MiB filesystem ceiling. A full log tmpfs can drop diagnostics; it cannot grow onto the root filesystem. Docker logging uses `none`; inspect diagnostics with `docker compose exec bot sh -c 'cat /var/log/sol/bot*'`. Extractor/decoder stderr is discarded; provider failure details intentionally stay generic to avoid exposing stream URLs or credentials. Health writes remain separate from logs.

The `--healthcheck` binary reads a gateway-readiness timestamp and fails after 90 seconds without renewal. Startup/reconnect readiness controls renewal. Songbird reconnects transient voice problems; an unrecoverable client error exits the process. Docker does not restart containers solely because a healthcheck fails. SIGTERM stops both sources and removes the heartbeat; restart policy is `unless-stopped`.

Container tmpfs may reach disk through host swap unless the kernel/cgroups enforce the memory/swap equality limit. Verify Docker inspect/cgroup swap settings and disable microSD-backed host swap; RAM-only zram is an option. Configure host journald/daemon logs for volatile RAM retention if the strict zero-transient-microSD-write target applies. Host settings have not been changed. [Docker tmpfs](https://docs.docker.com/engine/storage/tmpfs/) and [Compose swap limits](https://docs.docker.com/reference/compose-file/services/#memswap_limit).

Before production: inspect mounted tmpfs and LogConfig.Type=none, confirm read-only root writes fail, test tmpfs-full behavior, stop/restart cleanup, interrupted provider requests and pause/skip during overlap. Record decoded outbound audio for crossfade continuity and measure CPU, RSS/cgroup memory, temperature and underruns on the actual Orange Pi.

Public provider pages are allowlisted and initial stream DNS addresses reject private/local ranges. FFmpeg permits only HTTP/TLS transport protocols. Redirects, HLS child URLs and DNS rebinding are not comprehensively isolated by these application checks; use deployment-level egress restrictions before exposing an untrusted public bot at scale.
