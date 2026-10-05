FROM node:26-bookworm-slim AS provider
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/bgutil
# Pin both the provider revision and source archive hash. npm ci honors its lockfile.
RUN curl -fsSL https://codeload.github.com/Brainicism/bgutil-ytdlp-pot-provider/tar.gz/2df09aeaa71a4eec1e31901e84fbddbf0c7c54a9 -o /tmp/provider.tgz \
    && echo 'ae39fc64ac60e87c1f8ccada172f49471f7d5138f06e6302d84e9065786ad126  /tmp/provider.tgz' | sha256sum -c - \
    && tar -xzf /tmp/provider.tgz --strip-components=2 --wildcards '*/server' \
    && npm ci --no-audit --no-fund && npx tsc && npm prune --omit=dev \
    && rm /tmp/provider.tgz

FROM node:26-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates ffmpeg nftables util-linux python3 python3-venv \
    && python3 -m venv /opt/extractor \
    && /opt/extractor/bin/pip install --no-cache-dir \
      'yt-dlp[default]==2026.8.19' 'bgutil-ytdlp-pot-provider==2.0.1' \
    && groupadd --gid 10001 sol && useradd --uid 10001 --gid sol --no-create-home sol \
    && rm -rf /var/lib/apt/lists/*
ENV PATH="/opt/extractor/bin:${PATH}" PYTHONDONTWRITEBYTECODE=1 HOME=/tmp \
    XDG_CACHE_HOME=/tmp/cache TMPDIR=/tmp TOKEN_TTL=6
COPY --from=provider /opt/bgutil /opt/bgutil
COPY media /opt/media
RUN chmod 0555 /opt/media/entrypoint.sh
# Root is used only to establish the namespace firewall, then setpriv drops it.
ENTRYPOINT ["/opt/media/entrypoint.sh"]
