You are an expert backend engineer, DevOps specialist, and Discord API developer. Your task is to architect and develop a multi-purpose Discord bot suite, starting with a highly advanced music module. 

Review the project environment, constraints, and feature requirements below.

1. Hardware and Deployment Environment
The bot will run in a resource-constrained, ARM64 edge-computing environment. You must design the architecture and containerization strategy accordingly.
Hardware: Orange Pi Zero 3 (Allwinner H618 quad-core 64-bit Cortex-A53 at 1.5 GHz, Mali-G31 MP2 GPU, 4GB LPDDR4 RAM).
Storage: SanDisk Ultra 128GB 140MB/s microSDXC UHS-I (SDSQUAB-128G-GN6MN).
OS: DietPi OS (Lightweight Debian-based).
Deployment: Fully containerized using Docker and Docker Compose. Use restart: unless-stopped.
Storage Protection: To prevent premature SD card failure, configure Docker to mount tmpfs volumes directly into the containers. All temporary audio buffers, transcode caches, and high-frequency log writes MUST be kept entirely in RAM.
Audio Processing: Voice processing (e.g., FFmpeg, Opus) will happen inside the Docker containers. Ensure lightweight native dependencies are selected to handle concurrent audio streams efficiently without exhausting the 4GB RAM or CPU.

2. Core Development Standards
Modern Discord API: Strictly use the latest, non-deprecated libraries. Implement all commands as Application Slash Commands with modern UI components (Buttons, Modals, Select Menus).
Async Patterns: Ensure modern asynchronous patterns are used throughout to prevent blocking the event loop during network requests or audio buffering.
Dynamic Bot Identity: Implement dynamic identity resolution upon initialization. When joining or operating in a server, the bot must fetch and display its custom guild-specific server nickname. If no nickname is set, it should gracefully fall back to its default global Application Name from the Discord Developer Portal.
Intelligent Search Resolution: The bot must have a reliable system to pick the correct song when a user searches using a plain text query instead of a direct link. The search algorithm should prioritize official audio tracks, high-quality studio versions, or verified platform releases, effectively filtering out irrelevant covers, live versions, parodies, or ambient noise videos unless explicitly requested by the user.

3. Phase 1: Music Module Specifications
Begin by implementing the core audio bot. It must support the following platforms: YouTube, YouTube Music, SoundCloud, Deezer, Apple Music, Bandcamp, and Spotify.

Required Commands and Features:
/play [query] [position] [shuffle]: Accepts a search query or URL. 
- position: Next (adds to top of queue), Now (skips current and plays immediately).
- shuffle: True/False (starts shuffled if a playlist link is provided).
/pause: Pauses or resumes playback.
/skip: Skips the current song.
/stop: Stops playback and clears the queue.
/nowplaying: Displays detailed metadata about the current track.
/join and /disconnect: Manages voice channel presence.
Queue Management: /remove [position], /move [from] [to], /clear.
/playlist [subcommand]: create (from current song or queue), add (current song or queue), remove, delete, list, show, rename.
/seek [time]: Accepts flexible input formats (e.g., 1:30, 90s, +15s, -10s). Replaces separate forward/rewind commands.
/queue: Displays the current music queue.
/shuffle: Shuffles the queue.
/loop: Toggles repeat mode (current song vs. entire queue).
/volume: Adjusts playback volume.
/lyrics: Fetches lyrics for the current song.
/filter: Applies audio effects or EQ presets.
/search: Interactive search for tracks across supported platforms.
/history and /replay: Views recently played tracks or replays the last song.
/crossfade [mode: enabled/disabled] [seconds: 3 to 10]: Implements true, smooth audio crossfading by overlapping and blending two audio tracks simultaneously (like Spotify). Support customizable transition modes.
/settings: Configures server defaults (DJ role, default volume, 24/7 mode, announcement channel, auto-leave, etc.).
Utilities: /stats, /ping, and /help.

4. Your Execution Steps
Before writing any code, execute the following steps in order:

1. Research and Stack Proposal: Based on the current ecosystem, propose the absolute best programming language and wrapper library. 
2. Audio Engine Selection: Propose the best standalone audio node or internal library that specifically supports true track overlapping for the /crossfade requirement while remaining efficient enough for ARM64 hardware. Explain how this engine will handle the Intelligent Search Resolution to ensure accurate song parsing.
3. Architecture and Docker Outline: Provide the docker-compose.yml demonstrating the tmpfs logging/buffering implementation and dependency orchestration.
4. Implementation Kickoff: Once the stack is confirmed, begin outputting the directory structure and the foundational code for the dynamic identity resolution, the intelligent /play command, and the /crossfade mechanics.