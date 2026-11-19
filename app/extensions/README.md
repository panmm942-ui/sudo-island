# Built-in Spotify connector

Sudo includes its own small current-player reader, `spotify-client-adapter.js`, and source template, `sudo-spotify-bridge.js`. They do not require Spicetify. The adapter finds Spotify’s existing React platform provider and resolves only PlayerAPI and AuthorizationAPI. A live test on the original Spotify1.3.3 interface, with no Spicetify global, supplied Canvas and31timed lyric lines for the current song. Tracked templates contain no usable pairing key.

The bridge sends the current Spotify track URI, song metadata, playback position, original client cover-image URLs, an available Canvas URL and optional line-synced client lyrics to `127.0.0.1:43893`. It also accepts catalogue searches and individual track selections initiated in the Island. A selection asks Spotify's running player to start the track and checks the resulting URI and playback state. Spotify access tokens stay inside Spotify. The localhost receiver requires its local pairing key, the exact Spotify client origin and bounded validated packets. It rechecks the current Windows song before publishing media.

The connector uses native player updates plus a one-second poll. It uses Spotify's own `canvas.url` only when its `canvas.entityUri` equals the current track URI. It prefers the original `image_xlarge_url` to smaller images. If current-track Canvas metadata is absent, bounded requests may ask Spotify's Canvas endpoint for that track. Transient failures retry with backoff and respect Spotify's Retry-After deadline. Fresh valid metadata clears obsolete failures.

Choose **Settings → Music → Set up Spotify connection**. Setup currently supports the classic Windows desktop download from spotify.com. It writes the Sudo connector and its script tag, keeps a restore backup, and restarts Spotify once. If the original player is in xpui.spa, Sudo expands it with path, size, entry-count and symlink guards, preserving the original archive.

**Repair Spotify connection** reinstalls the connector after Spotify updates. **Restore previous Spotify setup** restores the backup only if the page still matches Sudo’s installed version, refusing to overwrite later unrelated changes. Original packages/backups remain on disk. Disabling closes the localhost listener; ordinary Windows media controls and fallbacks remain available. Microsoft Store-only installations are not supported by this setup yet. An existing Spicetify renderer remains compatible, but the built-in adapter is preferred and duplicate bridge instances are prevented. The old CLI-dependent installer was removed.

Verify the source extension without Spotify or network access:

```powershell
. F:\sudo\dev.ps1
Set-Location F:\sudo\app
node --check extensions/sudo-spotify-bridge.js
node scripts/test-spotify-client.mjs
node scripts/test-spotify-bridge.mjs
node scripts/test-spotify-commands.mjs
```

Actual availability of each song's Canvas, images and synced lyrics must be checked with the user's running Spotify client after optional activation. Synthetic tests do not establish live client availability.
