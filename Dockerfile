FROM rust:1.99-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake pkg-config libopus-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM node:24-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg libopus0 python3 python3-venv \
    && python3 -m venv /opt/extractor \
    && /opt/extractor/bin/pip install --no-cache-dir 'yt-dlp[default]==2026.8.19' \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 sol && useradd --uid 10001 --gid sol --no-create-home sol
ENV PATH="/opt/extractor/bin:${PATH}" PYTHONDONTWRITEBYTECODE=1 HOME=/tmp XDG_CACHE_HOME=/tmp/cache TMPDIR=/tmp
COPY --from=build /build/target/release/soldiscordbot /usr/local/bin/soldiscordbot
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/soldiscordbot"]

