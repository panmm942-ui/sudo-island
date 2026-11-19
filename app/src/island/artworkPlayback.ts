import type { default as Hls, LoadPolicy } from 'hls.js';
import { validArtworkUrl } from './artwork';
import { selectVideoLevel, type VideoQuality } from './videoQuality';

/** Detect a source that produces neither a frame nor an error. A hidden
 * decoder is allowed to wait until its view is opened again. */
export function watchVideoStartup(video: HTMLVideoElement, onTimeout: () => void, timeoutMs = 15_000) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let active = false;
  const clear = () => clearTimeout(timer);
  video.addEventListener('loadeddata', clear);
  return {
    setActive(next: boolean) {
      if (next === active) return;
      active = next;
      clear();
      if (active && video.readyState < 2) {
        timer = setTimeout(() => {
          if (video.readyState < 2) onTimeout();
        }, timeoutMs);
      }
    },
    stop() {
      active = false;
      clear();
      video.removeEventListener('loadeddata', clear);
    },
  };
}

/** The caller owns play/pause and loadeddata. Disposing always stops all loading.
 * This loads an ordinary HLS player on demand, with demuxing off the UI thread.
 */
export function attachArtworkVideo(video: HTMLVideoElement, url: string, onerror: () => void = () => {}, quality: VideoQuality = 'best'): () => void {
  let disposed = false;
  let failed = false;
  let player: Hls | undefined;
  function clear() {
    player?.destroy(); player = undefined;
    video.removeEventListener('error', fail);
    video.pause(); video.removeAttribute('src'); video.load();
  }
  function fail() {
    if (disposed || failed) return;
    failed = true; clear(); onerror();
  }
  if (!validArtworkUrl(url)) { queueMicrotask(fail); }
  else void import('hls.js').then(({ default: Hls }) => {
    if (disposed || failed) return;
    // Chromium can advertise native HLS yet stall on Apple's master playlist.
    // Prefer the MSE implementation; native HLS remains a Safari fallback.
    if (!Hls.isSupported()) {
      if (video.canPlayType('application/vnd.apple.mpegurl')) {
        video.addEventListener('error', fail);
        video.src = url;
        video.load();
      }
      else fail();
      return;
    }
    const retry = { maxNumRetry: 1, retryDelayMs: 500, maxRetryDelayMs: 500 };
    const bounded: LoadPolicy = {
      default: {
        maxTimeToFirstByteMs: 5_000, maxLoadTimeMs: 6_000, timeoutRetry: retry,
        errorRetry: {
          ...retry,
          shouldRetry: (_config, _count, timeout, response, allowed) => {
            const status = response?.code;
            return allowed && (timeout || !status || status === 408 || status >= 500);
          },
        },
      },
    };
    player = new Hls({
      enableWorker: true,
      autoStartLoad: false,
      capLevelToPlayerSize: false,
      maxBufferLength: 12,
      maxMaxBufferLength: 12,
      maxBufferSize: 8_000_000,
      backBufferLength: 0,
      manifestLoadPolicy: bounded,
      playlistLoadPolicy: bounded,
      fragLoadPolicy: bounded,
      keyLoadPolicy: { default: { ...bounded.default, timeoutRetry: null, errorRetry: null } },
      xhrSetup(xhr, requestUrl) {
        const source = new URL(requestUrl);
        if (source.protocol !== 'https:' || source.hostname !== 'mvod.itunes.apple.com' || source.port || source.username || source.password || !source.pathname.startsWith('/itunes-assets/')) throw new Error('Untrusted artwork media source.');
        xhr.withCredentials = false;
      },
    });
    let recoveredMedia = false;
    player.on(Hls.Events.ERROR, (_event, data) => {
      if (!data.fatal || disposed || failed || !player) return;
      if (data.type === Hls.ErrorTypes.MEDIA_ERROR && !recoveredMedia) {
        recoveredMedia = true;
        player.recoverMediaError();
      } else fail();
    });
    player.on(Hls.Events.MANIFEST_PARSED, () => {
      if (!player) return;
      // HLS has already filtered unsupported codecs. Start with the best playable
      // source, rather than stretching a low-resolution rendition across the card.
      const best = selectVideoLevel(player.levels, quality);
      if(best < 0){fail();return;}
      player.startLevel = best;
      player.loadLevel = best;
      player.startLoad();
    });
    player.attachMedia(video);
    player.loadSource(url);
  }).catch(fail);
  return () => { if (!disposed) { disposed = true; clear(); } };
}
