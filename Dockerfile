FROM rust:1.99-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake pkg-config libopus-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libopus0 python3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 sol && useradd --uid 10001 --gid sol --no-create-home sol
ENV PYTHONDONTWRITEBYTECODE=1 HOME=/tmp XDG_CACHE_HOME=/tmp/cache TMPDIR=/tmp
COPY --from=build /build/target/release/soldiscordbot /usr/local/bin/soldiscordbot
COPY media/client.py /opt/media/client.py
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/soldiscordbot"]

