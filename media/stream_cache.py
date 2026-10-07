"""Short-lived selected streams, kept only in the protected worker's RAM."""
from collections import OrderedDict
import json
import math
import threading
import time
import urllib.parse

FIELDS = ('id', 'title', 'artist', 'uploader', 'channel', 'channel_is_verified',
          'availability', 'duration', 'is_live', 'live_status', 'ie_key',
          'extractor_key', 'url', 'webpage_url', 'http_headers')


class StreamCache:
    def __init__(self, clock=time.monotonic, wall=time.time, capacity=32, ttl=90):
        self.clock, self.wall = clock, wall
        self.capacity, self.ttl = capacity, ttl
        self.entries = OrderedDict()
        self.lock = threading.Lock()
        self.generation = 0

    def clear(self):
        with self.lock:
            self.entries.clear()
            self.generation += 1

    def get(self, query):
        with self.lock:
            item = self.entries.get(query)
            if item is None:
                return None
            if item[0] <= self.clock():
                del self.entries[query]
                return None
            self.entries.move_to_end(query)
            return json.loads(item[1])

    def put(self, query, info, generation):
        # Flat search/playlist entries have no playable selected stream.
        if (info.get('entries') is not None or info.get('is_live')
                or info.get('live_status') == 'is_live'
                or info.get('_type', 'video') != 'video'
                or not isinstance(info.get('format_id'), str)):
            return
        url = urllib.parse.urlsplit(info.get('url', ''))
        if url.scheme != 'https':
            return
        ttl = self.ttl
        expiry = urllib.parse.parse_qs(url.query).get('expire')
        if expiry:
            try:
                remaining = float(expiry[0]) - self.wall() - 60
                if not math.isfinite(remaining):
                    return
                ttl = min(ttl, remaining)
            except ValueError:
                return
        if not 0 < ttl <= self.ttl:
            return
        value = json.dumps({k: info[k] for k in FIELDS if k in info}, separators=(',', ':'))
        if len(value.encode()) > 64 * 1024:
            return
        keys = {query, info.get('webpage_url')}
        with self.lock:
            if generation != self.generation:
                return  # Never retain a result from the former WARP session.
            for key in keys:
                if isinstance(key, str):
                    self.entries[key] = (self.clock() + ttl, value)
                    self.entries.move_to_end(key)
            while len(self.entries) > self.capacity:
                self.entries.popitem(last=False)
