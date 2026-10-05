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
this change does not claim measured DietPi or live YouTube latency.

After rebuilding both images, try a direct link once, then repeat it within 90
seconds. Search adds a separate search/selection stage. Read the existing logs:

```sh
docker compose exec --user 10001 media tail -n 60 /var/log/sol/media.log
docker compose exec --user 10001 bot sh -c 'tail -n 60 /var/log/sol/bot*'
```

Worker events: `extraction_complete` reports extraction time;
`stream_cache_hit` confirms reuse; `stream_first_pcm` reports decoder startup.
Bot events: `voice_join_complete`, `play_resolution_complete`,
`playback_stream_resolved`, `playback_pcm_buffered` and `playback_prepared`
report milliseconds. `playback_prepared` is cumulative from track preparation;
other events measure their named stage, including waits. They do not measure
when a listener hears audio. Logs omit queries, signed URLs, tokens and IDs.

Offline regressions verify process reuse, flat/full mode switching, bounded
output, timeout recovery, cache expiry/bounds and one extraction for the direct
metadata-to-stream pair. ARM64 CI also loads the pinned yt-dlp in the persistent
server with a local extraction fixture and verifies inherited network isolation,
proxy reachability and dropped capabilities. Live playback remains a deployment
check rather than a CI claim.
