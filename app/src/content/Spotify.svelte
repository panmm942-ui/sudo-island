<script lang="ts">
  import { isSpotifyActionPending, musicService, NEUTRAL_COVER_TINT, type MediaAction, type SpotifyTrack, type Tint } from "../island/spotify";
  import { playbackSeconds } from "../island/lyricTiming";
  import spotifyIcon from "../assets/spotify-icon.svg";
  import PlayerSpectrum from "./PlayerSpectrum.svelte";
  import type { SpotifySpectrum, WaveStyle } from "../island/waveform";
  import {invoke} from '@tauri-apps/api/core';
  import Marquee from './Marquee.svelte';
  import ExtensionIcon from "./ExtensionIcon.svelte";

  let { minimal=false, marquee=true, track, seekBar=false, oncontrol, layout = "centered", pinned = false, onpin, onlyrics, onsearch, artworkVideo=false, showRepeat=false, showShuffle=false, songColor=NEUTRAL_COVER_TINT, waves=false, level=null, sensitivity=1.6, style='classic', reactivity=40, smoothness=65, reduceMotion=false, videoReady=false, videoBusy=false, videoError='', fullscreenActive=false, onvideo }: {
    fullscreenActive?:boolean;
    minimal?:boolean;
    seekBar?:boolean;
    videoReady?:boolean;
    videoBusy?:boolean;
    videoError?:string;
    onvideo?:(mode:'fullscreen')=>void;
    artworkVideo?:boolean;
    marquee?:boolean;
    waves?: boolean;
    level?: SpotifySpectrum | null;
    sensitivity?: number;
    style?: WaveStyle;
    reactivity?: number;
    smoothness?: number;
    reduceMotion?: boolean;
    showRepeat?: boolean;
    showShuffle?: boolean;
    songColor?: Tint;
    track: SpotifyTrack;
    oncontrol: (action: MediaAction) => Promise<boolean>;
    pinned?: boolean;
    onpin?: () => void;
    onlyrics?: () => void;
    layout?: "centered" | "artwork-left" | "cover-card" | "wide";
    onsearch?: () => void;
  } = $props();

  let pending = $state<MediaAction[]>([]);
  let error = $state("");
  const busy = $derived(pending.length > 0);
  const hasOptions = $derived(showRepeat || showShuffle);
  const songRgb = $derived(songColor.map(channel => Math.round(Math.max(0, Math.min(1, channel)) * 255)).join(" "));
  const repeatLabel = $derived(track.repeatMode === 'track' ? 'Repeat one' : track.repeatMode === 'context' ? 'Repeat all' : track.repeatMode === 'off' ? 'Repeat off' : 'Repeat unavailable');
  const actionStatus = $derived(error || videoError);
  let now = $state(Date.now());
  const hasTimeline = $derived(typeof track.position === 'number' && Number.isFinite(track.position) && typeof track.duration === 'number' && Number.isFinite(track.duration) && track.duration > 0);
  const livePosition = $derived(hasTimeline ? playbackSeconds(track, now) : 0);
  // While dragging, and until Spotify reports the new position, show the chosen time.
  let dragging = $state<number | null>(null);
  let seekTarget = $state<{ key: string; seconds: number; until: number } | null>(null);
  const position = $derived(dragging ?? (seekTarget && seekTarget.key === track.key && now < seekTarget.until && Math.abs(livePosition - seekTarget.seconds) > 1.5 ? seekTarget.seconds : livePosition));
  const progress = $derived(hasTimeline ? Math.max(0, Math.min(1, position / track.duration!)) : 0);
  const showTimeline = $derived(layout === 'wide' || seekBar);
  const canSeek = $derived(seekBar && track.canSeek !== false);
  const service = $derived(musicService(track));
  const appleMusic = $derived(track.provider === "appleMusic");
  async function seek(seconds: number) {
    dragging = null;
    if (!hasTimeline || !canSeek) return;
    const key = track.key;
    seekTarget = { key, seconds, until: Date.now() + 2500 };
    try {
      if (!appleMusic) void invoke('vocal_flush').catch(() => {});
      const accepted = await invoke<boolean>('spotify_seek', { seconds, trackKey: key });
      if (!accepted && track.key === key) { seekTarget = null; error = `${service} couldn’t jump to that moment.`; }
    } catch (cause) {
      seekTarget = null;
      if (track.key === key) error = cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : `${service} couldn’t jump to that moment.`;
    }
  }
  const timestamp = (seconds: number | null | undefined) => typeof seconds === 'number' && Number.isFinite(seconds) ? `${Math.floor(Math.max(0, seconds) / 60)}:${Math.floor(Math.max(0, seconds) % 60).toString().padStart(2, '0')}` : '–:––';
  $effect(() => {
    // The timeline advances only from a real Spotify position; paused/unknown states stay still.
    if (!showTimeline || !hasTimeline || !track.playing) return;
    now = Date.now();
    const timer = setInterval(() => { now = Date.now(); }, 100);
    return () => clearInterval(timer);
  });
  const trackIdentity=$derived(track.key);
  $effect(() => { void trackIdentity; error = ''; });
  async function control(action: MediaAction) {
    if (isSpotifyActionPending(action, pending)) return;
    const key = track.key;
    pending = [...pending, action];
    error = "";
    try {
      const accepted = await oncontrol(action);
      if (!accepted && track.key === key) error = `${service} couldn’t complete that action.`;
    } catch (cause) {
      if (track.key === key) error = cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : `${service} couldn’t complete that action.`;
    } finally { pending = pending.filter(value => value !== action); }
  }
</script>

<section class="player" class:minimal class:motion-background={artworkVideo} class:artwork-left={layout === "artwork-left"} class:cover-card={layout==='cover-card'} class:wide={layout==='wide'} class:with-spectrum={layout==='wide'&&waves} class:has-options={hasOptions} class:with-seek={seekBar&&layout!=='wide'} style:--song-rgb={songRgb} aria-label={`${service} now playing`}>
  <!-- Click anywhere that isn't a control (or the cover) to keep Spotify open, like Home. -->
  {#if onpin}<button type="button" class="pin-surface" aria-label={pinned ? `Unpin ${service}` : `Keep ${service} open`} aria-pressed={pinned} onclick={onpin}></button>{/if}
  {#if onsearch}
    <button type="button" class="search-toggle" data-spotify-search-entry aria-label="Search Spotify" onclick={onsearch}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>
    </button>
  {/if}
  <button type="button" class="artwork-button" class:artwork-hidden={artworkVideo} aria-label="Album cover" tabindex="-1">
  {#if artworkVideo}
    <span class="motion-artwork" aria-hidden="true"></span>
  {:else if track.artwork}
    <img class="artwork" src={track.artwork} width="92" height="92" alt="" />
  {:else}
    <div class="artwork placeholder" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="30" height="30" fill="currentColor"><path d="M9 5v11a3 3 0 1 1-2-2.83V6l13-3v11a3 3 0 1 1-2-2.83V3.46Z" /></svg>
    </div>
  {/if}
  {#if layout==='wide'}
    {#if appleMusic}
      <span class="cover-brand" aria-hidden="true"><ExtensionIcon name="music" /></span>
    {:else}
      <img class="cover-brand" src={spotifyIcon} alt="" width="16" height="16" aria-hidden="true" />
    {/if}
  {/if}
  </button>
  {#if layout==='wide'&&waves}
    <div class="player-spectrum"><PlayerSpectrum playing={track.playing} {waves} {level} {sensitivity} {style} {reactivity} {smoothness} {reduceMotion} /></div>
  {/if}
  <div class="details" aria-live="polite">
    {#if layout==='artwork-left'}
      <p class="title">{track.title || "No track selected"}</p>
      <p class="artist">{track.artist || service}</p>
    {:else}
      <p class="title"><Marquee text={track.title || "No track selected"} enabled={marquee} /></p>
      <p class="artist"><Marquee text={track.artist || service} enabled={marquee} /></p>
    {/if}
  </div>
  {#if showTimeline}
    <div class="timeline" class:seekable={canSeek} aria-label="Track timeline">
      <span class="timestamp">{hasTimeline ? timestamp(position) : '–:––'}</span>
      {#if canSeek}
        <input class="seek" type="range" min="0" max={hasTimeline ? track.duration! : 1} step="0.1" value={position} disabled={!hasTimeline} style:--fill={`${progress * 100}%`} aria-label="Song position" aria-valuetext={hasTimeline ? `${timestamp(position)} of ${timestamp(track.duration)}` : 'Playback position unavailable'}
          oninput={(event) => { dragging = event.currentTarget.valueAsNumber; }} onchange={(event) => void seek(event.currentTarget.valueAsNumber)} />
      {:else}
        <div class="progress" role="progressbar" aria-label="Song progress" aria-valuemin="0" aria-valuemax={hasTimeline ? track.duration! : undefined} aria-valuenow={hasTimeline ? Math.floor(position) : undefined} aria-valuetext={hasTimeline ? `${timestamp(position)} of ${timestamp(track.duration)}` : 'Playback position unavailable'}>
          <span class="progress-fill" style:transform={`scaleX(${progress})`}></span>
        </div>
      {/if}
      <span class="timestamp">{timestamp(track.duration)}</span>
    </div>
  {/if}
  <div class="transport" aria-label="Playback controls" aria-busy={busy}>
    {#if showShuffle}
      <button type="button" class="option-btn shuffle" class:enabled={track.shuffle===true} aria-label={track.shuffle==null?'Shuffle unavailable':track.shuffle?'Turn shuffle off':'Turn shuffle on'} aria-pressed={track.shuffle==null?undefined:track.shuffle} aria-disabled={isSpotifyActionPending('shuffle', pending)} disabled={!track.canShuffle || track.shuffle==null} onclick={()=>control('shuffle')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 6h2.5c4.5 0 8.5 12 13 12H21M17 14l4 4-4 4M3 18h2.5c1.7 0 3.5-1.8 5.2-4.1M13.3 10.1C15 7.8 16.8 6 18.5 6H21M17 2l4 4-4 4"/></svg>
      </button>
    {/if}
    <button type="button" class="pill-btn" aria-label="Previous track" aria-disabled={isSpotifyActionPending('previous', pending)} disabled={!track.canPrevious} onclick={() => control("previous")}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d={layout==='wide'?"M12 5 2 12l10 7V5Zm10 0-10 7 10 7V5Z":"M18 5 8 12l10 7zM5 5h2v14H5z"} /></svg>
    </button>
    <button type="button" class="pill-btn toggle" aria-label={track.playing ? "Pause" : "Play"} aria-disabled={isSpotifyActionPending('toggle', pending)} disabled={!track.canToggle} onclick={() => control("toggle")}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d={track.playing ? "M6 4h4v16H6zM14 4h4v16h-4z" : "M8 4v16l12-8z"} /></svg>
    </button>
    <button type="button" class="pill-btn" aria-label="Next track" aria-disabled={isSpotifyActionPending('next', pending)} disabled={!track.canNext} onclick={() => control("next")}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d={layout==='wide'?"m2 5 10 7-10 7V5Zm10 0 10 7-10 7V5Z":"m6 5 10 7-10 7zM17 5h2v14h-2z"} /></svg>
    </button>
    {#if showRepeat}
      <button type="button" class="option-btn repeat" class:enabled={track.repeatMode==='context'||track.repeatMode==='track'} aria-label={repeatLabel} aria-pressed={track.repeatMode==null?undefined:track.repeatMode!=='off'} aria-disabled={isSpotifyActionPending('repeat', pending)} disabled={!track.canRepeat || track.repeatMode==null} onclick={()=>control('repeat')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m17 2 4 4-4 4M3 10V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4M21 14v1a3 3 0 0 1-3 3H3"/></svg>
        {#if track.repeatMode==='track'}<span class="repeat-one" aria-hidden="true">1</span>{/if}
      </button>
    {/if}

  </div>
  {#if actionStatus}<p class="action-status" role="status">{actionStatus}</p>{/if}
  {#if onvideo}
    <div class="video-actions" aria-label="Video views">
      <button type="button" aria-label={fullscreenActive?'Close fullscreen now playing':'Fullscreen now playing'} aria-pressed={fullscreenActive} disabled={videoBusy} onclick={()=>onvideo?.('fullscreen')}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/></svg></button>
    </div>
  {/if}
  {#if onlyrics}
    <button type="button" class="lyrics-toggle" aria-label="Show lyrics" onclick={onlyrics}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11.5 6.8a4 4 0 1 1 5.7 5.7l-2.1 2.1-5.7-5.7zM10.4 9.9l-6.3 7.4a1.4 1.4 0 0 0 .1 1.9l.6.6a1.4 1.4 0 0 0 1.9.1l7.4-6.3"/></svg>
    </button>
  {/if}
</section>

<style>
  /* The pin surface fills the panel behind everything; controls and the cover sit above it and
     text lets clicks through, so a click on any empty spot or text keeps Spotify open. */
  .pin-surface {position:absolute;inset:0;width:100%;height:100%;padding:0;border:0;border-radius:inherit;background:transparent;color:inherit;cursor:pointer;z-index:0;}
  .player :is(.artwork-button,.transport,.search-toggle,.video-actions,.lyrics-toggle,.timeline.seekable) {z-index:1;}
  .player .transport {position:relative;}
  .player :is(.details,.timeline:not(.seekable),.player-spectrum,.action-status) {pointer-events:none;}
  .video-actions {position:absolute;left:22px;bottom:14px;display:flex;gap:2px;}
  .video-actions button {width:28px;height:28px;display:grid;place-items:center;padding:5px;border:0;border-radius:8px;background:transparent;color:inherit;cursor:pointer;}
  .video-actions button:hover {background:var(--surface-control-hover);}
  .video-actions button:disabled {opacity:.3;cursor:default;}
  .video-actions svg {width:18px;height:18px;}
  .cover-card .video-actions,.wide .video-actions {bottom:24px;}
  .player { display: grid; justify-items: center; align-content: center; gap: var(--space-2); height: 100%; padding: var(--space-3) 64px; }
  .search-toggle { position:absolute;top:14px;left:22px;width:28px;height:28px;padding:5px;display:grid;place-items:center;border:0;border-radius:50%;background:transparent;color:inherit;cursor:pointer; }
  .search-toggle:hover {background:var(--surface-control-hover);}
  .search-toggle svg {width:18px;height:18px;}
  .artwork { display: block; width: 92px; height: 92px; object-fit: cover; border-radius: 12px; box-shadow: 0 6px 20px oklch(0.1 0 0 / 0.36); }
  .placeholder { display: grid; place-items: center; background: var(--surface-control); }
  .details { width: 100%; min-width: 0; text-align: center; }
  p { margin: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .title { font: var(--type-title); }
  .artist { font: var(--type-caption); }
  .transport { display: flex; align-items: center; gap: var(--space-2); }
  .transport button { width: 40px; height: 36px; padding: 0; }
  .transport .toggle { width: 46px; height: 40px; background: var(--surface-control-hover); }
  .transport button:disabled { opacity: 0.42; cursor: default; }
  .transport svg { width: 20px; height: 20px; fill: currentColor; }
  .artwork-button { position: relative; padding: 0; border: 0; border-radius: 12px; background: transparent; color: inherit; cursor: pointer; width: 92px; height: 92px; }
  /* The cover's white ring shows while it is pressed; it no longer marks "kept open". */
  .artwork-button:not(.artwork-hidden):active { box-shadow: 0 0 0 1px #ffffff75, 0 0 12px #ffffff35; }
  .lyrics-toggle { position: absolute; right: 22px; bottom: 14px; display: grid; place-items: center; width: 28px; height: 28px; padding: 5px; border: 0; border-radius: var(--radius-pill); background: transparent; color: inherit; cursor: pointer; }
  .lyrics-toggle:hover { background: var(--surface-control-hover); }
  .lyrics-toggle svg { width: 18px; height: 18px; }
  .artwork-left { grid-template-columns: 132px minmax(0, 1fr); grid-template-rows: 1fr auto; align-content: initial; align-items: center; justify-items: start; column-gap: 22px; row-gap: 10px; padding: 44px 66px 28px 28px; }
  .artwork-left .artwork-button { grid-row: 1 / 3; width: 132px; height: 132px; border-radius: 16px; }
  .artwork-left .artwork { width: 132px; height: 132px; border-radius: 16px; }
  .artwork-left .details { align-self: end; text-align: left; }
  .artwork-left .title { font-size: 18px; line-height: 23px; white-space: normal; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; }
  .artwork-left .transport { align-self: start; gap: 4px; }
  .artwork-left .transport button { width: 34px; }
  .artwork-left .transport .toggle { width: 42px; }
  .cover-card{position:relative;padding:0;display:block;}
  .cover-card::before{content:"";position:absolute;left:0;right:0;bottom:0;height:142px;background:linear-gradient(180deg,rgb(var(--song-rgb) / .18),rgb(var(--song-rgb) / .64));backdrop-filter:blur(14px);pointer-events:none;}
  .cover-card .artwork-button{position:absolute;inset:0 0 auto;width:100%;height:278px;border-radius:0;overflow:hidden;}
  .cover-card .artwork{width:100%;height:100%;object-fit:cover;border-radius:0;box-shadow:none;}
  .cover-card .artwork-button:not(.artwork-hidden):active{box-shadow:inset 0 0 0 1px #ffffff65;}
  .cover-card .details{position:absolute;left:58px;right:58px;width:auto;bottom:78px;}
  .cover-card .title{font:650 25px/31px var(--font-sans);letter-spacing:-.02em;}
  .cover-card .artist{font:550 18px/24px var(--font-sans);opacity:.76;margin-top:2px;}
  .cover-card .transport{position:absolute;left:50%;bottom:20px;transform:translateX(-50%);gap:10px;}
  .cover-card .transport button{background:transparent;width:34px;height:34px;}
  .cover-card .transport svg{width:24px;height:24px;}
  .cover-card .search-toggle{z-index:2;background:#00000020;backdrop-filter:blur(8px);}
  .cover-card .lyrics-toggle{bottom:25px;}
  .motion-artwork{display:block;width:100%;height:100%;}
  .player:not(.cover-card) { position: relative; }
  .transport .option-btn { position:relative;display:grid;place-items:center;flex:none;width:28px;height:32px;padding:5px;border:0;border-radius:var(--radius-pill);background:transparent;color:inherit;cursor:pointer; }
  .transport .option-btn:hover { background:var(--surface-control-hover); }
  .transport .option-btn svg { width:18px;height:18px;fill:none; }
  .transport .option-btn.enabled::after { content:'';position:absolute;bottom:0;left:calc(50% - 1.5px);width:3px;height:3px;border-radius:50%;background:currentColor; }
  .repeat-one { position:absolute;left:0;right:0;top:8px;font:650 9px/14px var(--font-sans);text-align:center; }
  .has-options .transport { gap:4px; }
  .has-options:not(.artwork-left):not(.wide):not(.cover-card) { padding-top:10px;padding-bottom:18px;gap:6px; }
  .artwork-left.has-options { grid-template-columns:96px minmax(0,1fr);grid-template-rows:96px 36px;column-gap:16px;row-gap:8px;padding:34px 64px 20px 28px; }
  .artwork-left.has-options .artwork-button { grid-row:1;width:96px;height:96px; }
  .artwork-left.has-options .artwork { width:96px;height:96px; }
  .artwork-left.has-options .details { align-self:center; }
  .artwork-left.has-options .transport { grid-column:1 / 3;justify-self:center;align-self:center; }
  .cover-card.has-options .transport { gap:7px; }
  .cover-card .transport .option-btn { width:28px;height:34px; }
  .cover-card .transport .option-btn svg { width:18px;height:18px; }
  .action-status { position:absolute;left:58px;right:64px;bottom:2px;text-align:center;font:500 10px/13px var(--font-sans); }
  .cover-card .action-status { bottom:5px; }
  .wide { display:block;padding:0; }
  .wide::before { content:'';position:absolute;inset:0;pointer-events:none;background:linear-gradient(130deg,rgb(var(--song-rgb) / .06),rgb(var(--song-rgb) / .28)); }
  .wide .artwork-button { position:absolute;left:28px;top:28px;width:96px;height:96px;border-radius:18px; }
  .wide .artwork { width:96px;height:96px;border-radius:18px; }
  .wide .cover-brand { position:absolute;left:7px;bottom:7px;width:16px;height:16px;border-radius:50%;filter:drop-shadow(0 1px 3px #0008);pointer-events:none; }
  .wide .cover-brand :global(svg) { width:100%;height:100%; }
  .wide .details { position:absolute;left:140px;right:64px;top:49px;width:auto;text-align:left; }
  .wide .title { font:650 21px/25px var(--font-sans);letter-spacing:var(--tracking-tight); }
  .wide .artist { font:450 16px/21px var(--font-sans);margin-top:3px; }
  .wide .search-toggle { left:auto;right:64px;top:14px; }
  .wide.with-spectrum .details { right:120px; }
  .player-spectrum { position:absolute;right:64px;top:58px;width:34px;height:30px; }
  .timeline { position:absolute;left:28px;right:64px;top:139px;display:flex;align-items:center;gap:10px; }
  .timestamp { flex:none;font:500 11px/16px var(--font-sans);font-variant-numeric:tabular-nums;min-width:28px; }
  .timestamp:last-child { text-align:right; }
  .progress { position:relative;height:5px;flex:1;overflow:hidden;border-radius:var(--radius-pill);background:var(--indicator-idle); }
  .progress-fill { position:absolute;inset:0;border-radius:inherit;background:#fff;transform-origin:left;transition:transform 100ms linear; }
  .wide .transport { position:absolute;left:28px;right:64px;bottom:20px;justify-content:center;gap:24px; }
  .wide .transport button { width:46px;height:44px;background:transparent; }
  .wide .transport button:hover { background:var(--surface-control-hover); }
  .wide .transport svg { width:30px;height:30px; }
  .wide .transport .toggle svg { width:34px;height:34px; }
  .wide.has-options .transport { gap:12px; }
  .wide .transport .option-btn { width:28px;height:32px; }
  .wide .transport .option-btn svg { width:18px;height:18px; }
  .wide .lyrics-toggle { bottom:24px; }
  @media (prefers-reduced-motion:reduce) { .progress-fill { transition:none; } }
  /* Draggable position bar: thin track, filled to the current time, thumb on hover. */
  .seek { color:#fff;flex:1;min-width:0;height:16px;margin:0;padding:0;background:transparent;cursor:pointer;-webkit-appearance:none;appearance:none; }
  .seek::-webkit-slider-runnable-track { height:5px;border-radius:var(--radius-pill);background:linear-gradient(90deg,#fff 0 var(--fill,0%),var(--indicator-idle) var(--fill,0%) 100%); }
  .seek::-webkit-slider-thumb { -webkit-appearance:none;appearance:none;width:13px;height:13px;margin-top:-4px;border-radius:50%;background:#fff;box-shadow:0 1px 4px #0006;opacity:0;transform:scale(.6);transition:opacity .15s,transform .15s; }
  .seek:hover::-webkit-slider-thumb,.seek:focus-visible::-webkit-slider-thumb,.seek:active::-webkit-slider-thumb { opacity:1;transform:none; }
  .seek:disabled { cursor:default;opacity:.5; }
  .seek:focus-visible { outline:none; }
  .seek:focus-visible::-webkit-slider-runnable-track { outline:2px solid var(--accent);outline-offset:3px; }
  .timeline.seekable { pointer-events:auto;z-index:1; }
  /* Placement of the bar in the non-wide layouts. */
  .with-seek:not(.artwork-left):not(.cover-card) .timeline { position:static;width:100%;max-width:300px; }
  .with-seek:not(.artwork-left):not(.cover-card) .artwork-button,.with-seek:not(.artwork-left):not(.cover-card) .artwork { width:64px;height:64px; }
  .with-seek:not(.artwork-left):not(.cover-card) { gap:4px; }
  .artwork-left.with-seek .timeline { position:absolute;left:182px;right:66px;bottom:64px;top:auto; }
  .artwork-left.has-options.with-seek .timeline { left:140px;bottom:62px; }
  .cover-card.with-seek .timeline { position:absolute;left:58px;right:58px;bottom:62px;top:auto; }
  .cover-card.with-seek .details { bottom:94px; }
  /* Fixed symmetric slots keep Play/Pause at the island centre with either option hidden. */
  .player .transport { display:grid;grid-template-columns:28px 40px 46px 40px 28px;align-items:center;justify-items:center;gap:8px; }
  .transport > [aria-label="Previous track"]{grid-column:2;grid-row:1;}
  .transport > .toggle{grid-column:3;grid-row:1;}
  .transport > [aria-label="Next track"]{grid-column:4;grid-row:1;}
  .transport > .shuffle{grid-column:1;grid-row:1;}
  .transport > .repeat{grid-column:5;grid-row:1;}
  .player.wide .transport,.player.cover-card .transport,.player.artwork-left .transport{left:50%;right:auto;transform:translateX(-50%);}
  .player.wide .transport{grid-template-columns:28px 46px 46px 46px 28px;gap:14px;}
  .player.cover-card .transport{grid-template-columns:28px 34px 34px 34px 28px;gap:10px;}
  .player.artwork-left .transport{position:absolute;bottom:18px;grid-template-columns:28px 34px 42px 34px 28px;gap:8px;}
  .player.artwork-left.has-options .transport{grid-column:auto;grid-row:auto;justify-self:auto;align-self:auto;}
  .artwork-left:not(.has-options){padding-top:18px;padding-bottom:60px;grid-template-rows:1fr;}
  .artwork-left:not(.has-options) .artwork-button{grid-row:1;}
  .artwork-left:not(.has-options) .details{align-self:center;}
  /* Right-click: everything but the cover, song, artist, song bar and playback buttons hides in
     place (keeping its space), so nothing that stays moves. */
  .player.minimal :is(.search-toggle,.player-spectrum,.video-actions,.lyrics-toggle) {visibility:hidden;pointer-events:none;}
</style>
