# Docker and DietPi deployment outline

Status: proposal, not a runnable deployment. The repository does not yet contain an application binary, Dockerfile, or published image. The [Compose outline](../docker-compose.yml) defines the desired runtime contract; its healthcheck and SOL_* variables must be implemented with the bot.

## Container and dependency orchestration

One ARM64 bot image includes the compiled Rust binary, native libopus, required FFmpeg tools, and pinned yt-dlp/EJS/JS runtime components. Use a multi-stage build and keep compilers/build caches out of the final image. Songbird is linked into the bot and SQLite is in-process; no external audio-node/database container or artificial depends_on entry is necessary. Startup checks tools, mounts and schema migrations before opening the Discord gateway. Native binaries execute from the read-only image, not /tmp.

The image runs as UID/GID 10001 and uses a read-only root filesystem. Before eventual deployment, create the data directory on DietPi and give it that owner. The bind mount deliberately has create_host_path disabled to avoid an automatically created root-owned directory. Only durable playlist/settings data belongs there.

## Preventing transient microSD writes

| Workload | Location | Maximum capacity |
| --- | --- | --- |
| Audio decode buffers | Bounded RAM buffers and pipes | Application admission/buffer limits |
| Media temporary files and extractor cache | /tmp and /tmp/cache, tmpfs | 96 MiB shared capacity |
| Application logs | /var/log/sol, tmpfs | 16 MiB filesystem; planned 4 x 2 MiB log rotation |
| Health/runtime state | /run/sol, tmpfs | 8 MiB |
| Saved playlists/settings | /data/sol.sqlite3 | Durable, low-frequency writes |

Docker normally records stdout/stderr independently of the application's log directory. The outline explicitly uses logging.driver=none. Application logging must use a bounded non-blocking writer and rotate inside tmpfs; /stats and the RAM log file provide operational visibility. docker logs intentionally returns no retained output. [Docker logging documentation](https://docs.docker.com/engine/logging/configure/).

tmpfs can be swapped to disk. The container sets memswap_limit equal to mem_limit to disallow its swap use; verify that the DietPi kernel/cgroup configuration actually enforces this. For the strict RAM-only target, configure the host without microSD-backed swap (RAM-only zram is an option) and verify the resulting configuration. Host journald and Docker daemon logs also need a RAM/volatile policy if frequent host logs would otherwise reach microSD. These host settings are separate from this Compose file and have not been changed. [Docker tmpfs behavior](https://docs.docker.com/engine/storage/tmpfs/), [Compose memory and swap controls](https://docs.docker.com/reference/compose-file/services/#memswap_limit).

tmpfs pages count inside the 1536 MiB container limit, not as an additional RAM allocation. The 120 MiB aggregate mount capacity is a cap, not preallocated memory. An audio decoder or cache must apply backpressure before memory exhaustion; no full playlist is downloaded in advance.

## Lifecycle and future healthcheck

Use restart: unless-stopped and a 20-second shutdown window to cancel extractors, stop both audio sources, release voice, and close durable transactions. The proposed --healthcheck mode reads a small heartbeat on /run/sol and checks its freshness/status without opening another Discord session. The application must also exit or reconnect after an unrecoverable fault: Docker marks a container unhealthy but does not restart it just for failing a healthcheck.

Keep command registration explicit and limited to a test guild during development. The runtime only needs outbound HTTPS/WebSocket/voice UDP, so the outline publishes no service ports. Provider secrets are supplied at deployment after their integrations are verified.

## Verification before deployment is considered working

1. Parse the outline and run docker compose --profile proposal config with placeholder values. This checks configuration, not image readiness.
2. Build a real linux/arm64 image; verify native Opus, DAVE, FFmpeg, extractor and JS runtime versions inside it.
3. Check Docker inspect for tmpfs mounts, LogConfig.Type=none, memory limits and MemorySwap equality. Check cgroup swap enforcement and host swap devices.
4. Confirm every write-intensive path is tmpfs/RAM and root filesystem writes fail. Ensure extraction uses /tmp and does not create a file-backed cache elsewhere.
5. Verify bounded log rotation, tmpfs-full handling, graceful shutdown and restart recovery.
6. Run a test-guild stream and repeated crossfade load on the actual Orange Pi, recording CPU, temperature, memory and underruns.

The development machine has no Docker runtime available. The proposal is YAML-validated locally; Docker config, image execution, voice authentication and target-device performance remain unverified until the implementation phase.
