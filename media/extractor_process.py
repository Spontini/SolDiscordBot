"""Reusable, bounded yt-dlp subprocess. A stalled session is killed and reaped."""
import json
import os
import queue
import signal
import subprocess
import sys
import threading
from common import event

LIMIT = 2 * 1024 * 1024


class ExtractorProcess:
    def __init__(self, command=None, timeout=65, limit=LIMIT):
        self.command = command or [sys.executable, '/opt/media/extract.py', '--serve']
        self.timeout, self.limit = timeout, limit
        self.lock = threading.Lock()
        self.serial = threading.Lock()
        self.session = None
        self.requests = 0

    def _reader(self, process, replies):
        try:
            while True:
                line = process.stdout.readline(self.limit + 1)
                if not line or len(line) > self.limit or not line.endswith(b'\n'):
                    raise ValueError('extractor_output_limit_or_eof')
                replies.put_nowait(line)
        except Exception:
            # No provider messages, URLs or visitor credentials enter logs.
            try:
                replies.put_nowait(None)
            except queue.Full:
                pass

    @staticmethod
    def _stop(process):
        if os.name == 'posix':
            try:
                # Also stop any JS runtime launched by this extractor.
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        elif process.poll() is None:
            process.kill()
        process.wait()
        process.stdin.close()
        process.stdout.close()

    def close(self):
        # Independent of serial: health checks can invalidate an active session.
        with self.lock:
            session, self.session = self.session, None
        if session:
            self._stop(session[0])

    def start(self):
        with self.lock:
            if self.session is None:
                process = subprocess.Popen(self.command, stdin=subprocess.PIPE,
                                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                           start_new_session=os.name == 'posix')
                replies = queue.Queue(maxsize=1)
                self.session = (process, replies)
                self.requests = 0
                threading.Thread(target=self._reader, args=self.session, daemon=True).start()
                event('extractor_session_started')
            return self.session

    def request(self, request):
        data = json.dumps({'query': request['query'], 'flat': request.get('flat') is True},
                          separators=(',', ':')).encode() + b'\n'
        if len(data) > 8192:
            raise ValueError('extractor_request_limit')
        with self.serial:
            if self.requests >= 64:
                self.close()  # Bound accumulated in-process extractor caches.
            session = self.start()
            process, replies = session
            try:
                process.stdin.write(data)
                process.stdin.flush()
                try:
                    line = replies.get(timeout=self.timeout)
                except queue.Empty:
                    raise TimeoutError('extractor_timeout') from None
                if line is None:
                    raise RuntimeError('extractor_protocol_failed')
                reply = json.loads(line)
                if reply.get('ok') is not True or not isinstance(reply.get('info'), dict):
                    raise RuntimeError('extractor_failed')
                with self.lock:
                    if self.session is not session:
                        raise RuntimeError('extractor_session_invalidated')
                    self.requests += 1
                return reply['info']
            except Exception:
                self.close()
                raise
