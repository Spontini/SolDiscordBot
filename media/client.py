"""Bot-side IPC only. No YouTube URLs, DNS lookups or redirect following."""
import ipaddress
import json
import os
import sys
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        raise ValueError('media_redirect_forbidden')


def endpoint():
    base = os.environ['SOL_MEDIA_ENDPOINT'].rstrip('/')
    url = urllib.parse.urlsplit(base)
    if (url.scheme != 'http' or url.username or url.password or url.path
            or url.query or url.fragment or url.port != 8080
            or not ipaddress.ip_address(url.hostname).is_private):
        raise ValueError('private_numeric_media_endpoint_required')
    return base


def open_local(url, data=None, timeout=80):
    base = endpoint()
    if url != base + '/health' and url != base + '/extract' and not url.startswith(base + '/pcm/'):
        raise ValueError('unexpected_media_destination')
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    return opener.open(urllib.request.Request(url, data=data,
                       headers={'Content-Type': 'application/json'}), timeout=timeout)


def main():
    action = sys.argv[1]
    if action == 'health':
        with open_local(endpoint() + '/health', timeout=5) as response:
            if response.read(1024) != b'{"ready":true}':
                raise RuntimeError('worker_not_ready')
    elif action == 'extract':
        request = {'query': sys.argv[3], 'flat': sys.argv[2] == 'flat'}
        with open_local(endpoint() + '/extract', json.dumps(request).encode()) as response:
            body = response.read(2 * 1024 * 1024 + 1)
        if len(body) > 2 * 1024 * 1024:
            raise ValueError('metadata_limit')
        sys.stdout.buffer.write(body)
    elif action == 'pcm':
        with open_local(sys.argv[2], timeout=90) as response:
            if response.headers.get('Content-Type') != 'application/octet-stream':
                raise ValueError('unexpected_media_type')
            while chunk := response.read(7680):
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
    else:
        raise ValueError('unknown_action')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('Protected media worker unavailable; direct playback is disabled.', file=sys.stderr)
        sys.exit(1)
