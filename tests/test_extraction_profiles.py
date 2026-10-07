"""Fast-profile compatibility and credential-free HTTP measurements."""
import io
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'media'))
from extract import extract, fast_video
from request_timings import RequestTimings, bucket, safe_metrics

URL = 'https://www.youtube.com/watch?v=fixture1234'
VIDEO = {'title': 'Fixture', 'format_id': '251', 'url': 'https://example.org/audio'}


class Session:
    def __init__(self, results):
        self.params = {'extractor_args': {'youtube': {'player_client': ['mweb']}}}
        self.results, self.calls = iter(results), []

    def extract_info(self, query, download):
        self.calls.append({k: list(v) for k, v in self.params['extractor_args']['youtube'].items()})
        result = next(self.results)
        if isinstance(result, Exception):
            raise result
        return result

    def sanitize_info(self, info):
        return info


class Profiles(unittest.TestCase):
    def test_video_detection_preserves_search_playlist_live_and_other_providers(self):
        for query in (URL, 'https://youtu.be/fixture1234',
                      'https://music.youtube.com/watch?v=fixture1234',
                      'https://www.youtube.com/shorts/fixture1234'):
            self.assertTrue(fast_video(query, True))
        for query in ('ytsearch10:song', URL + '&list=playlist',
                      'https://youtube.com/live/fixture1234',
                      'https://youtube.com/playlist?list=playlist',
                      'https://soundcloud.com/artist/song', 'https://youtube.com/watch?v=bad'):
            self.assertFalse(fast_video(query, True))
        self.assertTrue(fast_video(URL + '&list=playlist', False))

    def test_fast_success_keeps_attestation_client_and_restores_next_request(self):
        session = Session([VIDEO, {'entries': []}])
        self.assertEqual(extract({'query': URL, 'flat': True}, session), VIDEO)
        self.assertEqual(session._sol_profile, 'fast')
        self.assertEqual(session.calls[0]['player_skip'], ['configs', 'initial_data'])
        self.assertEqual(session.calls[0]['skip'], ['hls', 'dash'])
        self.assertEqual(session.calls[0]['player_client'], ['mweb'])
        extract({'query': 'ytsearch10:song', 'flat': True}, session)
        self.assertNotIn('skip', session.calls[1])
        self.assertNotIn('player_skip', session.calls[1])

    def test_live_missing_formats_and_errors_use_full_profile(self):
        for miss in (dict(VIDEO, is_live=True), dict(VIDEO, live_status='is_upcoming'),
                     {'title': 'Missing formats'}, RuntimeError('upstream secret')):
            session = Session([miss, VIDEO])
            self.assertEqual(extract({'query': URL}, session), VIDEO)
            self.assertEqual(session._sol_profile, 'fast_fallback')
            self.assertEqual(session.calls[1], {'player_client': ['mweb']})

    def test_double_failure_restores_options(self):
        session = Session([RuntimeError(), RuntimeError()])
        session.params['extractor_args']['youtube']['skip'] = ['dash']
        with self.assertRaises(RuntimeError):
            extract({'query': URL}, session)
        self.assertEqual(session.params['extractor_args']['youtube'],
                         {'player_client': ['mweb'], 'skip': ['dash']})


class Timings(unittest.TestCase):
    def test_open_and_body_reads_are_counted_without_changing_response(self):
        now = [0.0]
        class Response(io.BytesIO):
            def read(self, *args):
                now[0] += 0.2
                return super().read(*args)
        response = Response(b'private token body')
        class Downloader:
            def urlopen(self, request):
                now[0] += 0.1
                return response
        session = Downloader()
        tracker = RequestTimings(session, clock=lambda: now[0])
        self.assertIs(session.urlopen(URL + '&secret=do-not-log'), response)
        self.assertEqual(response.read(), b'private token body')
        self.assertEqual(tracker.snapshot(), {'webpage': {'requests': 1, 'elapsed_ms': 300, 'failures': 0}})
        tracker.reset()
        response.read()
        self.assertEqual(tracker.snapshot(), {})

    def test_failures_record_only_counts(self):
        class Downloader:
            def urlopen(self, request):
                raise RuntimeError('signed URL and token secret')
        session = Downloader()
        tracker = RequestTimings(session)
        with self.assertRaises(RuntimeError):
            session.urlopen('https://www.youtube.com/youtubei/v1/player?token=secret')
        self.assertEqual(tracker.snapshot()['player_api']['failures'], 1)
        self.assertNotIn('secret', str(tracker.snapshot()))

    def test_fixed_labels_and_parent_validation(self):
        self.assertEqual(bucket('http://127.0.0.1:4417/get_pot?visitor=secret'), 'token_broker')
        self.assertEqual(bucket('https://www.youtube.com/s/player/secret/base.js'), 'player_js')
        self.assertEqual(bucket('https://m.youtube.com/?token=secret'), 'client_config')
        self.assertEqual(bucket('https://manifest.googlevideo.com/secret'), 'manifest')
        values = {'requests': 1, 'elapsed_ms': 5, 'failures': 0}
        safe = safe_metrics({'profile': 'secret', 'http': {'webpage': values, 'secret': values,
                            'player_api': dict(values, elapsed_ms='secret'),
                            'player_js': dict(values, requests=True)}})
        self.assertEqual(safe, {'profile': 'full', 'http': {'webpage': values}})


if __name__ == '__main__':
    unittest.main()
