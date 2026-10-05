"""BgUtils adapter: session/video binding, conservative TTL and local failover.

yt-dlp owns VisitorData discovery and passes the matching InnerTube context.
Do not manufacture a VisitorData string or treat a video ID as VisitorData.
"""
import datetime
import hashlib
import json
import threading
import time
import urllib.request
from collections import OrderedDict

from common import event, run_bounded

VERSION = '2.0.1'
LEASE_SECONDS = 3600  # Reauthorize long playback every hour in worker.py.
MARGIN_SECONDS = 300


class TokenBroker:
    def __init__(self, proxy, primary=None, fallback=None, clock=time.monotonic,
                 wall=time.time, emit=event):
        self.proxy = proxy
        self.clock, self.wall, self.emit = clock, wall, emit
        self.primary = primary or self._primary
        self.fallback = fallback or self._fallback
        self.lock = threading.Lock()  # Single flight; no simultaneous cold generators.
        self.cache = OrderedDict()
        self.primary_retry_at = 0
        self.force_fallback = False
        self.epoch = 0

    def invalidate(self, *, fallback=False):
        with self.lock:
            self.cache.clear()
            self.epoch += 1
            self.force_fallback = fallback
            self.emit('token_cache_invalidated', fallback=fallback)

    def get(self, request):
        binding = request.get('content_binding')
        context = request.get('innertube_context')
        if not isinstance(binding, str) or not 1 <= len(binding) <= 8192:
            raise ValueError('missing_content_binding')
        if context is not None and not isinstance(context, dict):
            raise ValueError('invalid_context')
        # Ignore caller-selected routing, source addresses and TLS overrides.
        payload = {'content_binding': binding, 'innertube_context': context,
                   'challenge': request.get('challenge'), 'proxy': self.proxy,
                   'disable_tls_verification': False, 'bypass_cache': True}
        key = hashlib.sha256(json.dumps([binding, context, self.proxy, self.epoch],
                                        sort_keys=True).encode()).digest()
        with self.lock:
            cached = self.cache.get(key)
            if (not request.get('bypass_cache') and cached
                    and cached[1] - self.clock() > LEASE_SECONDS + MARGIN_SECONDS):
                self.cache.move_to_end(key)
                self.emit('token_cache_hit', remaining_seconds=int(cached[1] - self.clock()))
                return dict(cached[0])
            started = self.clock()
            response = None
            if not self.force_fallback and self.clock() >= self.primary_retry_at:
                try:
                    response = self._validate(self.primary(payload), binding)
                    self.emit('token_primary_success')
                except Exception as exc:
                    self.primary_retry_at = self.clock() + 60
                    self.emit('token_primary_failed', error_type=type(exc).__name__,
                              status=getattr(exc, 'code', None), retry_seconds=60)
            if response is None:
                self.emit('token_fallback_started')
                try:
                    response = self._validate(self.fallback(payload), binding)
                except Exception as exc:
                    self.emit('token_fallback_failed', error_type=type(exc).__name__)
                    raise RuntimeError('both_token_providers_unavailable') from None
                self.emit('token_fallback_success')
            self.force_fallback = False
            token, remaining = response
            self.cache[key] = (token, self.clock() + remaining)
            self.cache.move_to_end(key)
            while len(self.cache) > 256:
                self.cache.popitem(last=False)
            self.emit('token_refreshed', elapsed_ms=int((self.clock() - started) * 1000),
                      ttl_seconds=int(remaining))
            return dict(token)

    def _validate(self, response, binding):
        if not isinstance(response, dict):
            raise ValueError('invalid_token_response')
        token = response.get('poToken')
        if not isinstance(token, str) or not 1 <= len(token) <= 16384:
            raise ValueError('invalid_token')
        if response.get('contentBinding') != binding:
            raise ValueError('mismatched_binding')
        expiry = datetime.datetime.fromisoformat(response['expiresAt'].replace('Z', '+00:00'))
        if expiry.tzinfo is None:
            raise ValueError('ambiguous_expiry')
        # Provider TTL is an estimate, not an authorization guarantee from YouTube.
        remaining = min(expiry.timestamp() - self.wall(), 6 * 3600)
        if remaining <= LEASE_SECONDS + MARGIN_SECONDS:
            raise ValueError('insufficient_token_lifetime')
        return {k: response[k] for k in ('poToken', 'contentBinding', 'expiresAt')}, remaining

    @staticmethod
    def _primary(payload):
        # Explicitly disable environment proxies for this loopback-only IPC request.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        req = urllib.request.Request('http://127.0.0.1:4416/get_pot',
                                     data=json.dumps(payload).encode(),
                                     headers={'Content-Type': 'application/json'})
        with opener.open(req, timeout=5) as response:
            data = response.read(128 * 1024 + 1)
        if len(data) > 128 * 1024:
            raise ValueError('provider_output_limit')
        return json.loads(data)

    @staticmethod
    def _fallback(payload):
        command = ['node', '/opt/bgutil/build/generate_once.js', '--content-binding',
                   payload['content_binding'], '--proxy', payload['proxy'], '--bypass-cache']
        if payload['innertube_context'] is not None:
            command += ['--innertube-context', json.dumps(payload['innertube_context'])]
        # The one-shot generator has independent process state. Its stdout can
        # contain warnings, so parse just its final JSON line and never log it.
        raw = run_bounded(command, timeout=12, limit=128 * 1024)
        return json.loads(raw.splitlines()[-1])
