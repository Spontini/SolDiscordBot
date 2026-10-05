# Playback startup

The protected media worker keeps a serial yt-dlp process alive, reusing its
imports, extractors and HTTP sessions. It starts at worker boot. Sessions rotate
after 64 extractions and are killed/reaped on timeout, extraction failure or a
change of WARP egress address. Proxy and token-provider settings are fixed.

For a direct link, metadata resolution may already select a playable stream.
The worker retains only selected fields in RAM for up to 90 seconds, bounded
to 32 lookup keys and 64 KiB per entry. The original link and canonical webpage
URL share the result, so the following stream request avoids a second extraction.
Signed URL expiry imposes an additional 60-second safety margin. Search and
playlist results, live streams and oversized entries are not cached. A stream
retry or periodic lease refresh clears the cache and extractor session before
fresh-token extraction. Egress changes also reject in-flight cache insertions.

This cache does not contain audio. CDN URLs and headers remain inside the
protected worker; the bot still receives single-use private PCM tickets.
Token caching/failover, mandatory WARP, TLS checks and the namespace firewall
continue to apply. Cold provider/network latency can still exceed two seconds;
these changes do not guarantee sub-two-second live playback.

The first DietPi measurement after session/cache reuse still took about 18
seconds for a direct URL: 15,255 ms resolution and 2,791 ms preparation.
The selected stream was cached, so repeated extraction was no longer the
problem. The 2,520 ms token refresh was only part of that resolution time.

Single YouTube video links now first try an extraction profile that skips
the extra client configuration fetch, initial-data fallback, and HLS/DASH
manifests. It retains the watch page, mweb client, matching visitor/token
provider and JavaScript solving. Live videos, missing selected audio formats
and extraction errors retry with the full profile. Searches, playlist pages
and other providers start with the full profile; settings are restored after
every request. A fast-profile miss does not itself force token failover.
These options are documented in the [yt-dlp extractor arguments](https://github.com/yt-dlp/yt-dlp#extractor-arguments).

For the latest extraction-profile/timing update, rebuild only the media image;
the bot already contains the stage logs. Try a direct link once, then repeat
it within 90 seconds. Search adds a separate search/selection stage. Read:

```sh
docker compose exec --user 10001 media tail -n 60 /var/log/sol/media.log
docker compose exec --user 10001 bot sh -c 'tail -n 60 /var/log/sol/bot*'
```

Worker events: `extraction_complete` reports extraction time;
`stream_cache_hit` confirms reuse; `stream_first_pcm` reports decoder startup.
`extraction_http_timings` identifies the profile (`fast`, `fast_fallback`,
`full`) and fixed HTTP buckets with request counts, failures and milliseconds
spent opening responses and reading bodies. Bucket totals can overlap when
requests run concurrently; they are not a sum of exclusive wall-clock stages.
They omit local JS execution and parsing. `warp_unavailable` includes a safe
underlying exception type and elapsed time to distinguish connect failures
from timeouts without retaining error messages or addresses.
Bot events: `voice_join_complete`, `play_resolution_complete`,
`playback_stream_resolved`, `playback_pcm_buffered` and `playback_prepared`
report milliseconds. `playback_prepared` is cumulative from track preparation;
other events measure their named stage, including waits. They do not measure
when a listener hears audio. Logs omit queries, signed URLs, tokens and IDs.

Offline regressions verify process reuse, flat/full mode switching, bounded
output, timeout recovery, cache expiry/bounds and one extraction for the direct
metadata-to-stream pair. ARM64 CI also loads the pinned yt-dlp in the persistent
server with a local extraction fixture and verifies inherited network isolation,
proxy reachability, actual Response timing wrappers, profile switching and
dropped capabilities. Profile regressions cover live/missing-format/error
fallback and restoring settings for the next provider. Live playback and the
speed gain from the fast profile remain deployment checks rather than CI claims.
