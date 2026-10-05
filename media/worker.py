"""Private extraction/PCM service; its entire network namespace is firewalled.

Signed CDN URLs and visitor context stay here. Discord receives metadata and
single-use PCM tickets. FFmpeg and both BgUtils paths use the same WARP egress.
"""
import ipaddress
import json
import math
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from common import configure_logging, event, run_bounded
from extract import validate_query
from tokens import LEASE_SECONDS, TokenBroker, VERSION

PROXY = os.environ.get('SOL_MEDIA_PROXY', 'http://172.30.90.1:40001')
WORKER_IP = os.environ.get('SOL_WORKER_IP', '172.30.90.2')
BOT_IP = os.environ.get('SOL_BOT_IP', '172.30.90.3')
BROKER = TokenBroker(PROXY)
EXTRACT_GATE = threading.Lock()
DECODERS = threading.BoundedSemaphore(2)
TICKETS = {}
TICKET_LOCK = threading.Lock()
WARP_LOCK = threading.Lock()
WARP = {'checked': 0, 'ready': False, 'ip': None}


def warp_ready():
    """Verify the tunnel, not merely the existence of a listening proxy socket."""
    with WARP_LOCK:
        if time.monotonic() - WARP['checked'] < 30:
            return WARP['ready']
        try:
            opener = urllib.request.build_opener(urllib.request.ProxyHandler(
                {'http': PROXY, 'https': PROXY}))
            with opener.open('https://www.cloudflare.com/cdn-cgi/trace', timeout=5) as response:
                trace = dict(line.split('=', 1) for line in response.read(8192).decode().splitlines()
                             if '=' in line)
            ready = trace.get('warp') in ('on', 'plus') and bool(trace.get('ip'))
            if not ready:
                raise RuntimeError('warp_not_active')
            if WARP['ip'] != trace['ip']:
                BROKER.invalidate()
                # Bypass-cache requests also prevent reuse of primary process tokens.
                WARP['ip'] = trace['ip']
                event('warp_egress_changed')
        except Exception as exc:
            ready = False
            event('warp_unavailable', error_type=type(exc).__name__)
        WARP.update(checked=time.monotonic(), ready=ready)
        return ready


def extract(request, refresh=False, failsafe=False):
    validate_query(request['query'])
    if not warp_ready():
        raise RuntimeError('warp_unavailable')
    if not EXTRACT_GATE.acquire(timeout=5):
        raise RuntimeError('extractor_busy')
    try:
        if refresh:
            BROKER.invalidate(fallback=failsafe)
        for attempt in range(2):
            try:
                raw = run_bounded(['python3', '/opt/media/extract.py'],
                                  data=json.dumps(request).encode(), timeout=65)
                return json.loads(raw)
            except Exception as exc:
                event('extraction_failed', attempt=attempt + 1, error_type=type(exc).__name__)
                if attempt:
                    raise
                BROKER.invalidate(fallback=True)
                event('extraction_retry_fresh_tokens')
    finally:
        EXTRACT_GATE.release()


def ffmpeg_command(info, offset=0):
    url = info.get('url', '')
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != 'https' or parsed.username or parsed.password
            or parsed.port not in (None, 443) or not parsed.hostname):
        raise ValueError('https_stream_required')
    # Prevent media URLs from targeting private literal addresses. DNS is resolved
    # by WARP; there is no worker-side external DNS exception in the firewall.
    try:
        literal = ipaddress.ip_address(parsed.hostname)
    except ValueError:
        literal = None
    if literal is not None and not literal.is_global:
        raise ValueError('private_stream_address')
    headers = ''
    for name in ('User-Agent', 'Referer', 'Origin'):
        value = info.get('http_headers', {}).get(name)
        if value:
            if not isinstance(value, str) or len(value) > 2048 or '\r' in value or '\n' in value:
                raise ValueError('invalid_stream_header')
            headers += f'{name}: {value}\r\n'
    # HTTPS through an HTTP CONNECT proxy opens FFmpeg's nested httpproxy
    # transport. Omitting it rejects the stream before the first PCM byte.
    command = ['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error',
               '-threads', '1', '-rw_timeout', '15000000', '-http_proxy', PROXY,
               '-tls_verify', '1', '-ca_file', '/etc/ssl/certs/ca-certificates.crt',
               '-protocol_whitelist', 'http,https,httpproxy,tcp,tls,crypto']
    if headers:
        command += ['-headers', headers]
    if offset:
        command += ['-ss', str(offset)]
    return command + ['-i', url, '-vn', '-ac', '2', '-ar', '48000', '-f', 'f32le', 'pipe:1']


def put_ticket(request, info):
    ticket = uuid.uuid4().hex
    with TICKET_LOCK:
        now = time.monotonic()
        for key in list(TICKETS):
            if TICKETS[key][2] < now:
                del TICKETS[key]
        if len(TICKETS) >= 8:
            raise RuntimeError('too_many_pending_streams')
        TICKETS[ticket] = (request, info, now + 90)
    return f'http://{WORKER_IP}:8080/pcm/{ticket}'


def public_metadata(info):
    """Remove CDN addresses, headers, visitor data and PO tokens from IPC metadata."""
    result = {k: info[k] for k in ('id', 'title', 'artist', 'uploader', 'channel',
              'channel_is_verified', 'availability', 'duration', 'is_live',
              'live_status', 'ie_key', 'extractor_key') if k in info}
    for key in ('url', 'webpage_url'):
        try:
            validate_query(info.get(key))
            result[key] = info[key]
        except (ValueError, TypeError):
            pass
    if isinstance(info.get('entries'), list):
        result['entries'] = [public_metadata(e) for e in info['entries'][:200] if isinstance(e, dict)]
    return result


class LimitedServer(ThreadingHTTPServer):
    daemon_threads = True
    def __init__(self, *args):
        self.slots = threading.BoundedSemaphore(8)
        super().__init__(*args)

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()


class Handler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(90)

    def log_message(self, *_):
        pass  # HTTP paths contain private playback tickets.

    def reply(self, status, value):
        data = json.dumps(value, separators=(',', ':')).encode()
        if len(data) > 2 * 1024 * 1024:
            raise ValueError('response_limit')
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        length = int(self.headers.get('Content-Length', '0'))
        if not 1 <= length <= 128 * 1024:
            raise ValueError('invalid_request_size')
        return json.loads(self.rfile.read(length))

    def do_POST(self):
        try:
            if self.server.server_port == 4417 and self.path == '/get_pot':
                self.reply(200, BROKER.get(self.body()))
            elif self.client_address[0] == BOT_IP and self.path == '/extract':
                request = self.body()
                info = extract(request)
                if not request.get('flat'):
                    # Only local PCM tickets leave this worker, never CDN URLs/headers.
                    duration = info.get('duration')
                    if info.get('is_live') or info.get('live_status') == 'is_live':
                        duration = None
                    elif not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration <= 0:
                        duration = None
                    info = {'url': put_ticket(request, info), 'duration': duration,
                            'is_live': duration is None}
                else:
                    info = public_metadata(info)
                self.reply(200, info)
            else:
                self.reply(403, {'error': 'forbidden'})
        except Exception as exc:
            event('request_failed', error_type=type(exc).__name__)
            self.reply(503, {'error': 'protected_media_unavailable'})

    def do_GET(self):
        if self.server.server_port == 4417 and self.path == '/ping':
            self.reply(200, {'version': VERSION})
        elif self.path == '/health' and self.client_address[0] in (BOT_IP, WORKER_IP, '127.0.0.1'):
            self.reply(200 if warp_ready() else 503, {'ready': WARP['ready']})
        elif self.client_address[0] == BOT_IP and self.path.startswith('/pcm/'):
            self.pcm(self.path.removeprefix('/pcm/'))
        else:
            self.reply(403, {'error': 'forbidden'})

    def pcm(self, ticket):
        with TICKET_LOCK:
            item = TICKETS.pop(ticket, None)
        if not item or item[2] < time.monotonic():
            self.reply(410, {'error': 'expired_ticket'})
            return
        if not DECODERS.acquire(blocking=False):
            self.reply(503, {'error': 'decoder_busy'})
            return
        request, info, _ = item
        sent = 0
        started = False
        failures = 0
        live = info.get('is_live') or info.get('live_status') == 'is_live'
        try:
            while True:
                if not warp_ready():
                    raise RuntimeError('warp_unavailable')
                offset = 0 if live else sent / (48000 * 2 * 4)
                command = ffmpeg_command(info, offset)
                # Periodic reauthorization: every <=1 hour of delivered PCM obtains
                # a fresh stream URL and token with >1h+5min estimated lifetime.
                segment_limit = LEASE_SECONDS * 48000 * 2 * 4
                segment_bytes = 0
                lease_deadline = time.monotonic() + LEASE_SECONDS
                with subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as decoder:
                    # Bound initial decoder stalls and cancellation cleanup.
                    timer = threading.Timer(30, decoder.kill)
                    timer.start()
                    try:
                        while segment_bytes < segment_limit and time.monotonic() < lease_deadline:
                            block = decoder.stdout.read(min(7680, segment_limit - segment_bytes))
                            if not block:
                                break
                            if not started:
                                self.send_response(200)
                                self.send_header('Content-Type', 'application/octet-stream')
                                self.end_headers()
                                # Paused playback applies TCP backpressure. Do not
                                # turn an intentional long pause into a write timeout.
                                # Closing the bot client still breaks the socket write.
                                self.connection.settimeout(None)
                                started = True
                            timer.cancel()
                            # Client disconnect kills/reaps decoder in finally.
                            self.wfile.write(block)
                            sent += len(block)
                            segment_bytes += len(block)
                        ended = segment_bytes < segment_limit and time.monotonic() < lease_deadline
                        if ended:
                            status = decoder.wait(timeout=5)
                        else:
                            status = 0
                    finally:
                        timer.cancel()
                        if decoder.poll() is None:
                            decoder.kill()
                        decoder.wait()
                if ended and status == 0 and segment_bytes:
                    return
                if ended:
                    failures += 1
                    if failures > 1:
                        raise RuntimeError('stream_retry_exhausted')
                    event('stream_retry_fresh_tokens', delivered_seconds=int(offset))
                else:
                    event('stream_lease_refresh', delivered_seconds=int(offset))
                info = extract(request, refresh=True, failsafe=ended)
        except (BrokenPipeError, ConnectionResetError, socket.timeout):
            event('stream_client_disconnected')
        except Exception as exc:
            event('stream_failed', error_type=type(exc).__name__)
            if not started:
                self.reply(503, {'error': 'protected_stream_unavailable'})
        finally:
            DECODERS.release()


def main():
    configure_logging()
    primary = subprocess.Popen(['node', '/opt/bgutil/build/main.js', '--host',
                                '127.0.0.1', '--port', '4416'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    broker_server = LimitedServer(('127.0.0.1', 4417), Handler)
    threading.Thread(target=broker_server.serve_forever, daemon=True).start()
    # A crashed primary remains recoverable via the independent local generator.
    event('media_worker_started', provider_version=VERSION, lease_seconds=LEASE_SECONDS)
    try:
        LimitedServer(('0.0.0.0', 8080), Handler).serve_forever()
    finally:
        broker_server.shutdown()
        primary.terminate()
        primary.wait(timeout=5)


if __name__ == '__main__':
    main()
