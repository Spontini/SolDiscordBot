"""Serial yt-dlp session; the mandatory proxy covers every network handler."""
import json
import os
import sys
import urllib.parse


def validate_query(query):
    if not isinstance(query, str) or not 1 <= len(query) <= 512:
        raise ValueError('invalid_query')
    if query.startswith('ytsearch10:'):
        return
    url = urllib.parse.urlsplit(query)
    host = (url.hostname or '').lower()
    if (url.scheme not in ('https', 'http') or url.username or url.password or url.port
            or not any(host == h or host.endswith('.' + h)
                       for h in ('youtube.com', 'youtu.be', 'soundcloud.com', 'bandcamp.com'))):
        raise ValueError('unsupported_provider')


class QuietLogger:
    # yt-dlp debug output can expose signed URLs and tokens. Never retain it.
    def debug(self, *_): pass
    def info(self, *_): pass
    def warning(self, *_): pass
    def error(self, *_): pass


def downloader():
    import yt_dlp
    options = {
        'proxy': os.environ['SOL_MEDIA_PROXY'], 'cachedir': False,
        'quiet': True, 'no_warnings': True, 'logger': QuietLogger(),
        'skip_download': True, 'socket_timeout': 12, 'retries': 1,
        'extractor_retries': 1, 'js_runtimes': {'node': {}},
        'playlistend': 200, 'format': 'bestaudio/best',
        # The HTTP plugin calls our TTL/failover adapter, not the primary directly.
        # mweb + a provider is yt-dlp's recommended setup; web can be SABR-only.
        'extractor_args': {'youtube': {'player_client': ['mweb']},
                           'youtubepot-bgutilhttp': {'base_url': ['http://127.0.0.1:4417']}},
    }
    return yt_dlp.YoutubeDL(options)


def extract(request, session):
    validate_query(request['query'])
    flat = request.get('flat') is True
    # Only these two flags change; proxy, TLS and provider settings stay fixed.
    session.params.update(extract_flat='in_playlist' if flat else False, noplaylist=not flat)
    return session.sanitize_info(session.extract_info(request['query'], download=False))


def serve():
    # Reuse HTTP sessions and imported extractors. The parent serializes requests
    # and kills this process group on timeout, error, or WARP egress change.
    with downloader() as session:
        while True:
            line = sys.stdin.buffer.readline(8193)
            if not line:
                return
            if len(line) > 8192 or not line.endswith(b'\n'):
                raise ValueError('request_limit')
            try:
                info = extract(json.loads(line), session)
                reply = json.dumps({'ok': True, 'info': info}, separators=(',', ':'))
                if len(reply.encode()) + 1 > 2 * 1024 * 1024:
                    raise ValueError('response_limit')
            except Exception:
                reply = '{"ok":false}'
            print(reply, flush=True)


if __name__ == '__main__':
    try:
        if sys.argv[1:] == ['--serve']:
            serve()
        else:
            request = json.loads(sys.stdin.buffer.read(8192))
            with downloader() as session:
                print(json.dumps(extract(request, session), separators=(',', ':')))
    except Exception:
        # Parent logs a fixed failure class; credentials never reach stderr.
        sys.exit(1)
