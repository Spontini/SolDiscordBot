import concurrent.futures
import datetime
import json
import sys
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'media'))
from tokens import TokenBroker
from common import run_bounded
from extract import validate_query
from worker import ffmpeg_command, public_metadata
from client import endpoint, open_local


class Tokens(unittest.TestCase):
    def setUp(self):
        self.now = [100.0]
        self.events = []
        self.calls = []
        self.wall = 1700000000

    def response(self, request, remaining=21600):
        self.calls.append(request)
        expiry = datetime.datetime.fromtimestamp(self.wall + remaining, datetime.timezone.utc)
        return {'poToken': 'secret-token', 'contentBinding': request['content_binding'],
                'expiresAt': expiry.isoformat()}

    def broker(self, primary=None, fallback=None):
        return TokenBroker('http://172.30.90.1:40001', primary or self.response,
                           fallback or self.response, lambda: self.now[0], lambda: self.wall,
                           lambda name, **fields: self.events.append((name, fields)))

    def test_cache_and_refresh_margin(self):
        b = self.broker()
        b.get({'content_binding': 'visitor'})
        b.get({'content_binding': 'visitor'})
        self.assertEqual(len(self.calls), 1)
        self.now[0] += 21600 - 3900
        b.get({'content_binding': 'visitor'})
        self.assertEqual(len(self.calls), 2)

    def test_context_and_binding_are_not_shared(self):
        b = self.broker()
        for request in [{'content_binding': 'a'}, {'content_binding': 'b'},
                        {'content_binding': 'a', 'innertube_context': {'client': {'visitorData': 'v'}}}]:
            b.get(request)
        self.assertEqual(len(self.calls), 3)

    def test_primary_failure_and_circuit(self):
        def failed(_): raise TimeoutError()
        b = self.broker(primary=failed)
        b.get({'content_binding': 'a'})
        b.get({'content_binding': 'b'})
        names = [e[0] for e in self.events]
        self.assertEqual(names.count('token_primary_failed'), 1)
        self.assertEqual(names.count('token_fallback_success'), 2)
        self.assertNotIn('secret-token', json.dumps(self.events))

    def test_wrong_binding_uses_fallback(self):
        def wrong(request):
            response = self.response(request)
            response['contentBinding'] = 'other-video'
            return response
        b = self.broker(primary=wrong)
        self.assertEqual(b.get({'content_binding': 'video'})['contentBinding'], 'video')
        self.assertIn('token_fallback_success', [e[0] for e in self.events])

    def test_insufficient_expiry_uses_fallback(self):
        b = self.broker(primary=lambda request: self.response(request, 100))
        b.get({'content_binding': 'video'})
        self.assertIn('token_fallback_success', [e[0] for e in self.events])

    def test_both_fail_closed(self):
        def fail(_): raise RuntimeError('sensitive signed URL')
        with self.assertRaisesRegex(RuntimeError, '^both_token_providers_unavailable$'):
            self.broker(primary=fail, fallback=fail).get({'content_binding': 'v'})
        self.assertNotIn('sensitive', json.dumps(self.events))

    def test_invalidation_and_bypass(self):
        b = self.broker()
        b.get({'content_binding': 'v'})
        b.get({'content_binding': 'v', 'bypass_cache': True})
        b.invalidate(fallback=True)
        b.get({'content_binding': 'v'})
        self.assertEqual(len(self.calls), 3)

    def test_proxy_and_tls_cannot_be_overridden(self):
        b = self.broker()
        b.get({'content_binding': 'v', 'proxy': '', 'disable_tls_verification': True,
               'source_address': '127.0.0.1'})
        self.assertEqual(self.calls[0]['proxy'], 'http://172.30.90.1:40001')
        self.assertFalse(self.calls[0]['disable_tls_verification'])
        self.assertNotIn('source_address', self.calls[0])

    def test_single_flight(self):
        def slow(request):
            time.sleep(0.02)
            return self.response(request)
        b = self.broker(primary=slow)
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            list(pool.map(lambda _: b.get({'content_binding': 'v'}), range(8)))
        self.assertEqual(len(self.calls), 1)

    def test_cache_is_bounded(self):
        b = self.broker()
        for i in range(260): b.get({'content_binding': str(i)})
        self.assertEqual(len(b.cache), 256)


class Isolation(unittest.TestCase):
    def test_queries_cannot_supply_arbitrary_destinations(self):
        for value in ['http://127.0.0.1/', 'https://youtube.com.evil/x',
                      'file:///etc/passwd', 'https://user@youtube.com/v',
                      'https://youtube.com:123/v', '--proxy=']:
            with self.assertRaises(ValueError): validate_query(value)
        validate_query('ytsearch10:music')
        validate_query('https://www.youtube.com/watch?v=x')

    def test_ffmpeg_requires_proxy_and_https(self):
        command = ffmpeg_command({'url': 'https://cdn.googlevideo.com/stream'})
        self.assertIn('-http_proxy', command)
        self.assertEqual(command[command.index('-http_proxy') + 1], 'http://172.30.90.1:40001')
        for value in ['http://cdn.googlevideo.com/s', 'https://127.0.0.1/s',
                      'https://[::1]/s', 'file:///tmp/s']:
            with self.assertRaises(ValueError): ffmpeg_command({'url': value})
        with self.assertRaises(ValueError):
            ffmpeg_command({'url': 'https://cdn.googlevideo.com/s',
                            'http_headers': {'Origin': 'x\r\nEvil: y'}})

    def test_no_secrets_or_cdn_urls_in_metadata(self):
        info = {'title': 'song', 'url': 'https://cdn.googlevideo.com/private',
                'webpage_url': 'https://youtube.com/watch?v=x', 'poToken': 'secret',
                'http_headers': {'Cookie': 'secret'}, 'formats': [{'url': 'secret'}]}
        result = public_metadata(info)
        self.assertEqual(result, {'title': 'song', 'webpage_url': 'https://youtube.com/watch?v=x'})

    def test_client_cannot_open_external_address(self):
        with patch.dict('os.environ', {'SOL_MEDIA_ENDPOINT': 'http://172.30.90.2:8080'}):
            with self.assertRaises(ValueError): open_local('https://youtube.com/')
        with patch.dict('os.environ', {'SOL_MEDIA_ENDPOINT': 'http://8.8.8.8:8080'}):
            with self.assertRaises(ValueError): endpoint()

    def test_subprocess_output_is_bounded(self):
        with self.assertRaises(ValueError):
            run_bounded([sys.executable, '-c', 'print("x"*100000)'], limit=100)

    def test_subprocess_timeout_is_bounded(self):
        with self.assertRaises(Exception):
            run_bounded([sys.executable, '-c', 'import time;time.sleep(5)'], timeout=0.05)


if __name__ == '__main__': unittest.main()
