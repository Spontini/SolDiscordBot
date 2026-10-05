# Provider capabilities and intelligent resolution

Status: integration plan; no adapters implemented yet. A provider name in the requirements is not evidence of full-track playback access.

## Capability matrix

| Provider | Initial integration plan | Full-track Discord audio |
| --- | --- | --- |
| YouTube | URL/playlist metadata and bounded search/extraction via yt-dlp | Only where an accessible source works; authentication, regional and extraction failures are surfaced |
| YouTube Music | Recognize music.youtube.com links through the YouTube adapter | Same extraction constraints; no unsupported claim of a public Music catalog API |
| SoundCloud | Authorized API metadata/search and playable transcoded stream URLs | Only for tracks marked playable; preview/blocked tracks are distinguished |
| Bandcamp | Public track/album URL resolution through yt-dlp | Available public streams only, subject to extractor and track availability |
| Spotify | Catalog/link metadata where credentials and app access permit | No direct backend audio rebroadcast promised; matching another source requires an allowed integration and explicit source labeling |
| Apple Music | Catalog/link metadata through supported API access | MusicKit player access is not a raw backend stream; no direct rebroadcast promised |
| Deezer | Capability investigation, then catalog/link adapter if access is verified | Unverified; do not label previews as complete songs or invent a full-stream endpoint |

yt-dlp lists YouTube, SoundCloud and Bandcamp extractors, but a listed extractor is not a guarantee that a particular link currently works. YouTube extraction also requires EJS and a supported JS runtime. [Supported sites](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md), [EJS setup](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

SoundCloud documents playable, preview and blocked access, and restrictions on off-platform streaming. Include uploader attribution and a source link. [SoundCloud playback guide](https://developers.soundcloud.com/docs/api/guide#playing).

Spotify's platform policy imposes access and content-use requirements; metadata display must retain attribution and source links. Apple provides catalog APIs and MusicKit playback for supported clients, which does not establish a server-side decoded audio interface. Recheck the intended metadata-to-source matching against applicable provider access before enabling it. [Spotify policy](https://developer.spotify.com/policy), [Apple MusicKit](https://developer.apple.com/musickit/).

Deezer's API documentation redirected to an authenticated page during this research. Its endpoint availability and credentials are therefore unresolved, rather than claimed supported. [Deezer developer portal](https://developers.deezer.com/api).

## Search resolution design

The resolver, not the audio engine, decides which recording a query means. Songbird plays the result; it does not identify an official song by itself.

1. Recognize supported URLs first. An exact accessible track URL preserves the user's selection. Parse playlist identity separately and respect queue limits.
2. Normalize plain-text queries using Unicode case folding and token comparison, while retaining artist names and requested versions. Recognize intent such as live, cover, remix, karaoke or acoustic without blindly stripping those words from legitimate song titles.
3. Gather a bounded candidate set and enrich only the best candidates with title, artist/uploader, duration, release identifiers and provenance when available. Missing metadata stays unknown.
4. Score artist/title agreement, version agreement, duration proximity and trustworthy release provenance. Penalize unrequested covers/live/parody/ambient/extended versions. A verified uploader is a weak signal; the words Official Audio alone prove nothing.
5. Use ISRC/release metadata when present to distinguish recordings; compare duration to reject long music-video intros or different versions. Do not rank primarily by views or bitrate.
6. Auto-select only when both confidence and separation from the next candidate exceed calibrated thresholds. Otherwise return an expiring select menu with titles, sources and durations. Keep the user's explicit version request authoritative.
7. Resolve the chosen media URL shortly before decoding; never persist expiring URLs or provider tokens in a saved playlist.

Initial weights/thresholds are design parameters to calibrate against a labeled fixture set. Measure correct-recording selections and rejection of unwanted versions; do not describe heuristic scores as verified accuracy. Provider-wide interactive /search comes after the basic resolver and each adapter's capability checks.

## Process and URL handling

Spawn yt-dlp/FFmpeg with argument arrays and no shell interpolation. Enforce job timeouts, cancellation, response/output caps, and child-process cleanup. Restrict supported user URLs and validate redirects/media destinations against private/loopback/link-local addresses. Keep media buffers in bounded memory or the container's tmpfs. Refuse unavailable/protected media clearly rather than substituting a different recording without telling the user.
