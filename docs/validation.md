# Validation status

Date: 5 October 2026. Owner confirmed Rust + Serenity + Songbird before application code was started.

Local: Cargo formatting and dependency-lock resolution completed. Compose schema and relative-document links are checked separately. This Windows host has no Docker or Linux environment; Linux compilation/tests and native ARM64 container execution run in GitHub Actions.

CI checks: `cargo fmt --all --check`, `cargo test --locked`, strict Clippy, ARM64 Docker build, architecture inspection, and runtime FFmpeg/yt-dlp/Node checks. Final run results will be recorded after completion.

Behavior tests cover queue limits/order, version-intent ranking, Unicode normalization, confidence/ambiguity, supported-provider hosts, public-IP checks, live-duration handling, both input signals during transition gain mixing, stale request rejection, preparation cancellation and decoder process cleanup. Signal tests prove gain/mix policy, not live voice output continuity.

Not validated yet: Discord token authentication, command/component behavior in a real guild, current provider extraction reliability, recorded voice overlap and click/no-gap behavior, Orange Pi CPU/RAM/temperature, filesystem mount enforcement, host swap/log policies and prolonged failure recovery. No production-ready or benchmark claim is made.

Remaining gates are in [audio design](audio-engine.md) and [deployment](deployment.md).
