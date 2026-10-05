"""Serial yt-dlp session; the mandatory proxy covers every network handler."""
import json
import os
import re
import sys
import urllib.parse
from request_timings import RequestTimings


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
    # Proxy, TLS verification, attestation and JS solving remain mandatory.
    session.params.update(extract_flat='in_playlist' if flat else False, noplaylist=not flat)
    youtube = session.params['extractor_args']['youtube']
    saved = {key: youtube.get(key) for key in ('player_skip', 'skip')}
    session._sol_profile = 'full'
    try:
        if fast_video(request['query'], flat):
            # Ordinary songs have direct adaptive audio formats. Avoid a second
            # homepage/config fetch, metadata fallback, and manifest downloads.
            # Keep the watch page: it supplies the matching visitor identity.
            youtube.update(player_skip=['configs', 'initial_data'], skip=['hls', 'dash'])
            session._sol_profile = 'fast'
            try:
                info = session.extract_info(request['query'], download=False)
                if playable_video(info):
                    return session.sanitize_info(info)
            except Exception:
                pass  # A profile miss is not evidence of an invalid PO token.
            session._sol_profile = 'fast_fallback'
        restore_profile(youtube, saved)
        return session.sanitize_info(session.extract_info(request['query'], download=False))
    finally:
        # Do not let a fast request change later searches, playlists or providers.
        restore_profile(youtube, saved)


def restore_profile(options, saved):
    for key, value in saved.items():
        if value is None:
            options.pop(key, None)
        else:
            options[key] = value


def fast_video(query, flat):
    if query.startswith('ytsearch10:'):
        return False
    url = urllib.parse.urlsplit(query)
    host, path = (url.hostname or '').lower(), url.path.strip('/').split('/')
    params = urllib.parse.parse_qs(url.query)
    if flat and 'list' in params:
        return False
    if host == 'youtu.be':
        identity = path[0] if len(path) == 1 else ''
    elif host == 'youtube.com' or host.endswith('.youtube.com'):
        if path == ['watch']:
            identity = params.get('v', [''])[0]
        elif len(path) == 2 and path[0] in ('shorts', 'embed'):
            identity = path[1]
        else:
            return False  # Explicit live URLs and playlist pages use the full path.
    else:
        return False
    return re.fullmatch(r'[A-Za-z0-9_-]{11}', identity) is not None


def playable_video(info):
    return (isinstance(info, dict) and info.get('_type', 'video') == 'video'
            and not info.get('is_live') and info.get('live_status') not in ('is_live', 'is_upcoming')
            and isinstance(info.get('title'), str) and bool(info['title'].strip())
            and isinstance(info.get('format_id'), str)
            and isinstance(info.get('url'), str) and info['url'].startswith('https://'))


def serve():
    # Reuse HTTP sessions and imported extractors. The parent serializes requests
    # and kills this process group on timeout, error, or WARP egress change.
    with downloader() as session:
        timings = RequestTimings(session)
        while True:
            line = sys.stdin.buffer.readline(8193)
            if not line:
                return
            if len(line) > 8192 or not line.endswith(b'\n'):
                raise ValueError('request_limit')
            timings.reset()
            session._sol_profile = 'full'
            try:
                info = extract(json.loads(line), session)
                reply = json.dumps({'ok': True, 'info': info, 'profile': session._sol_profile,
                                    'http': timings.snapshot()}, separators=(',', ':'))
                if len(reply.encode()) + 1 > 2 * 1024 * 1024:
                    raise ValueError('response_limit')
            except Exception:
                reply = json.dumps({'ok': False, 'profile': session._sol_profile,
                                    'http': timings.snapshot()}, separators=(',', ':'))
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
