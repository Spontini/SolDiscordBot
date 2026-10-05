"""One extraction per process; the mandatory proxy covers every yt-dlp handler."""
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


def extract(request):
    import yt_dlp
    validate_query(request['query'])
    flat = request.get('flat') is True
    options = {
        'proxy': os.environ['SOL_MEDIA_PROXY'], 'cachedir': False,
        'quiet': True, 'no_warnings': True, 'logger': QuietLogger(),
        'skip_download': True, 'socket_timeout': 12, 'retries': 1,
        'extractor_retries': 1, 'js_runtimes': {'node': {}},
        'extract_flat': 'in_playlist' if flat else False, 'playlistend': 200,
        'noplaylist': not flat, 'format': 'bestaudio/best',
        # The HTTP plugin calls our TTL/failover adapter, not the primary directly.
        # mweb + a provider is yt-dlp's recommended setup; web can be SABR-only.
        'extractor_args': {'youtube': {'player_client': ['mweb']},
                           'youtubepot-bgutilhttp': {'base_url': ['http://127.0.0.1:4417']}},
    }
    with yt_dlp.YoutubeDL(options) as downloader:
        return downloader.sanitize_info(downloader.extract_info(request['query'], download=False))


if __name__ == '__main__':
    try:
        request = json.loads(sys.stdin.buffer.read(8192))
        print(json.dumps(extract(request), separators=(',', ':')))
    except Exception:
        # Parent logs a fixed failure class; credentials never reach stderr.
        sys.exit(1)
