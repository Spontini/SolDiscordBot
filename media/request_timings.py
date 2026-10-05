"""Measure HTTP open/body time without retaining upstream URLs or credentials."""
import time
import urllib.parse

BUCKETS = ('webpage', 'client_config', 'player_api', 'player_js', 'manifest',
           'search_api', 'token_broker', 'youtube_other', 'provider_other')
PROFILES = ('full', 'fast', 'fast_fallback')


def bucket(url):
    try:
        parsed = urllib.parse.urlsplit(url)
        host, path = (parsed.hostname or '').lower(), parsed.path
        if host == '127.0.0.1' and parsed.port == 4417:
            return 'token_broker'
        if host == 'manifest.googlevideo.com':
            return 'manifest'
        if host == 'youtube.com' or host.endswith('.youtube.com'):
            if path.startswith('/youtubei/v1/player'):
                return 'player_api'
            if path.startswith('/youtubei/v1/'):
                return 'search_api'
            if path.startswith('/s/player/') and path.endswith('.js'):
                return 'player_js'
            if path == '/':
                return 'client_config'
            if path == '/watch':
                return 'webpage'
            return 'youtube_other'
    except (TypeError, ValueError):
        pass
    return 'provider_other'


class RequestTimings:
    def __init__(self, session, clock=time.monotonic):
        self.clock = clock
        self.reset()
        original = session.urlopen

        def urlopen(request):
            name = bucket(request if isinstance(request, str) else getattr(request, 'url', ''))
            # Capture this request's counters; late reads cannot affect a later extraction.
            counters = self.stats.setdefault(name, {'requests': 0, 'seconds': 0.0, 'failures': 0})
            counters['requests'] += 1
            started = self.clock()
            try:
                response = original(request)
            except Exception:
                counters['failures'] += 1
                raise
            finally:
                counters['seconds'] += self.clock() - started
            read = response.read

            def timed_read(*args, **kwargs):
                started = self.clock()
                try:
                    return read(*args, **kwargs)
                except Exception:
                    counters['failures'] += 1
                    raise
                finally:
                    counters['seconds'] += self.clock() - started

            response.read = timed_read
            return response

        session.urlopen = urlopen

    def reset(self):
        self.stats = {}

    def snapshot(self):
        return {name: {'requests': item['requests'], 'elapsed_ms': round(item['seconds'] * 1000),
                       'failures': item['failures']} for name, item in self.stats.items()}


def safe_metrics(reply):
    """Allow only fixed labels and bounded integers across the child log boundary."""
    profile = reply.get('profile')
    result = {'profile': profile if profile in PROFILES else 'full', 'http': {}}
    metrics = reply.get('http')
    if isinstance(metrics, dict):
        for name in BUCKETS:
            item = metrics.get(name)
            if isinstance(item, dict) and all(type(item.get(k)) is int and 0 <= item[k] <= 1000000
                                             for k in ('requests', 'elapsed_ms', 'failures')):
                result['http'][name] = {k: item[k] for k in ('requests', 'elapsed_ms', 'failures')}
    return result
