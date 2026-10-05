# Development Roadmap

Current milestone: stack confirmed; Rust music foundation implemented and undergoing CI validation. Live Discord voice validation and Orange Pi benchmarks remain open. See [validation](validation.md) for evidence and limits.

## 1. Research and stack proposal

- Compare maintained languages and Discord wrappers against the current Discord voice and application-command requirements.
- Verify ARM64 availability of native FFmpeg and Opus dependencies.
- Investigate each provider's supported access methods; document direct playback versus metadata resolution.
- Propose a stack with evidence and an explicit resource budget for the target device.

## 2. Audio engine validation

- Demonstrate two simultaneous decoded tracks blended into one outbound stream.
- Confirm configurable 3â€“10 second transitions and transition modes.
- Measure CPU and RAM use on the deployment target, including overlapping tracks.
- Design intelligent query ranking and expose ambiguous matches through interactive selection.

## 3. Architecture and deployment outline

- Define module boundaries for Discord commands, guild state, provider adapters, audio mixing, and persistent playlists/settings.
- Provide Docker Compose with dependency orchestration and `restart: unless-stopped`.
- Put temporary audio, transcode caches, and high-frequency log writes on bounded tmpfs mounts.
- Disable or explicitly redirect Docker's persistent container logging; a tmpfs directory alone does not prevent Docker from writing stdout/stderr logs to disk.
- Keep durable settings and saved playlists separate from transient media and logs.
- Document memory limits, restart behavior, and cleanup.

## 4. Stack confirmation and implementation kickoff

Begin foundational code after the stack is confirmed, as required by the project brief:

- Dynamic guild identity resolution.
- Intelligent `/play` resolution and queue insertion.
- Actual overlapping `/crossfade` mechanics.
- Foundational directory structure and deployment configuration.

## 5. Music feature delivery

- Voice presence and playback: `/join`, `/disconnect`, `/play`, `/pause`, `/skip`, `/stop`, `/nowplaying`.
- Queue management: `/queue`, `/remove`, `/move`, `/clear`, `/shuffle`, `/loop`.
- Saved playlists: create, add, remove, delete, list, show, and rename.
- Playback enhancements: `/seek`, `/volume`, `/filter`, `/crossfade`.
- Discovery: `/search`, `/lyrics`, `/history`, `/replay`.
- Administration and utilities: `/settings`, `/stats`, `/ping`, `/help`.

## Validation gates

- Validate slash commands and interactive controls in a test guild.
- Verify seeking, queue ordering, playback cancellation, and graceful voice disconnects.
- Confirm crossfade continuity with recorded audio and resource measurements.
- Inspect mounts and logging behavior to ensure transient workloads remain in RAM.
- Document per-provider limitations and failures clearly.
