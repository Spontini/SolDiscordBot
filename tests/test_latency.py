"""Offline regressions for warm sessions and metadata-to-stream reuse."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'media'))
from extractor_process import ExtractorProcess
from stream_cache import StreamCache
import worker

URL = 'https://www.youtube.com/watch?v=fixture'
SHORT = 'https://youtu.be/fixture'
INFO = {'id': 'fixture', 'title': 'Fixture', 'webpage_url': URL,
        'duration': 120, 'url': 'https://cdn.googlevideo.com/audio?expire=1700001000',
        'http_headers': {'User-Agent': 'fixture'}, 'formats': [{'url': 'private'}]}


class Cache(unittest.TestCase):
    def setUp(self):
        self.now = [0]
        self.cache = StreamCache(clock=lambda: self.now[0], wall=lambda: 1700000000)

    def test_alias_reuse_and_expiry(self):
        self.cache.put(SHORT, INFO, 0)
        self.assertEqual(self.cache.get(URL)['url'], INFO['url'])
        self.assertNotIn('formats', self.cache.get(SHORT))
        self.now[0] = 90
        self.assertIsNone(self.cache.get(URL))

    def test_signed_expiry_margin_and_invalid_expiry(self):
        for expiry in ('1700000060', 'garbage', 'nan'):
            info = dict(INFO, url='https://cdn.googlevideo.com/audio?expire=' + expiry)
            self.cache.put(URL, info, 0)
            self.assertIsNone(self.cache.get(URL))
        self.cache.put(URL, dict(INFO, url='https://cdn.googlevideo.com/audio?expire=1700000070'), 0)
        self.now[0] = 10
        self.assertIsNone(self.cache.get(URL))

    def test_egress_invalidation_rejects_inflight_result(self):
        self.cache.put(URL, INFO, 0)
        self.cache.clear()
        self.cache.put(URL, INFO, 0)
        self.assertIsNone(self.cache.get(URL))

    def test_bounds_and_noncacheable_results(self):
        for i in range(100):
            self.cache.put(str(i), INFO, 0)
        self.assertLessEqual(len(self.cache.entries), 32)
        self.cache.clear()
        for info in ({'entries': []}, dict(INFO, is_live=True), dict(INFO, title='x' * 70000)):
            self.cache.put(URL, info, self.cache.generation)
            self.assertIsNone(self.cache.get(URL))

    def test_metadata_then_stream_uses_one_extraction_and_refresh_bypasses(self):
        with patch.object(worker, 'STREAMS', self.cache), \
                patch.object(worker, 'warp_ready', return_value=True), \
                patch.object(worker.EXTRACTOR, 'request', return_value=INFO) as request, \
                patch.object(worker.EXTRACTOR, 'close'), \
                patch.object(worker.BROKER, 'invalidate'):
            metadata = worker.public_metadata(worker.extract({'query': SHORT, 'flat': True}))
            self.assertEqual(metadata['webpage_url'], URL)
            self.assertNotIn('http_headers', metadata)
            stream = worker.extract({'query': URL, 'flat': False})
            self.assertEqual(stream['url'], INFO['url'])
            self.assertEqual(request.call_count, 1)
            worker.extract({'query': URL, 'flat': False}, refresh=True)
            self.assertEqual(request.call_count, 2)


class Sessions(unittest.TestCase):
    def process(self, body, timeout=2, limit=2048):
        script = 'import sys,json,os,time\nfor line in sys.stdin:\n r=json.loads(line)\n ' + body
        return ExtractorProcess([sys.executable, '-u', '-c', script], timeout=timeout, limit=limit)

    def test_reuses_process_and_restarts_after_close(self):
        session = self.process('print(json.dumps({"ok":True,"info":{"pid":os.getpid()}}),flush=True)')
        try:
            a = session.request({'query': URL})
            self.assertEqual(a, session.request({'query': SHORT}))
            session.close()
            self.assertNotEqual(a, session.request({'query': URL}))
        finally:
            session.close()

    def test_timeout_reaps_process_and_recovers(self):
        session = self.process('time.sleep(5) if r["query"] == "stall" else None; '
                               'print(json.dumps({"ok":True,"info":{}}),flush=True)', timeout=0.5)
        try:
            with self.assertRaises(TimeoutError):
                session.request({'query': 'stall'})
            self.assertIsNone(session.session)
            session.timeout = 2
            self.assertEqual(session.request({'query': URL}), {})
        finally:
            session.close()

    def test_output_limit_invalid_json_and_provider_failure_reset_session(self):
        for body in ('print("x"*10000,flush=True)', 'print("invalid",flush=True)',
                     'print("{\\"ok\\":false}",flush=True)', 'sys.exit(1)'):
            session = self.process(body)
            try:
                with self.assertRaises((ValueError, RuntimeError)):
                    session.request({'query': URL})
                self.assertIsNone(session.session)
            finally:
                session.close()

    def test_real_serve_protocol_keeps_downloader_and_switches_flat_mode(self):
        # Exercise extract.py's actual loop with a deterministic yt-dlp fixture.
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, 'yt_dlp.py').write_text('''
class YoutubeDL:
    def __init__(self, params): self.params = params; self.calls = 0
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def sanitize_info(self, info): return info
    def extract_info(self, query, download):
        self.calls += 1
        return {'calls': self.calls, 'flat': self.params['extract_flat'],
                'noplaylist': self.params['noplaylist'], 'proxy': self.params['proxy']}
''', encoding='utf-8')
            with patch.dict(os.environ, {'PYTHONPATH': directory, 'SOL_MEDIA_PROXY': 'http://172.30.90.1:40001'}):
                session = ExtractorProcess([sys.executable, '-u', str(Path(worker.__file__).with_name('extract.py')), '--serve'])
                try:
                    first = session.request({'query': URL, 'flat': True})
                    second = session.request({'query': URL, 'flat': False})
                    self.assertEqual(first['calls'], 1)
                    self.assertEqual(second['calls'], 2)
                    self.assertEqual(first['flat'], 'in_playlist')
                    self.assertFalse(second['flat'])
                    self.assertTrue(second['noplaylist'])
                    self.assertEqual(second['proxy'], 'http://172.30.90.1:40001')
                finally:
                    session.close()


if __name__ == '__main__': unittest.main()
