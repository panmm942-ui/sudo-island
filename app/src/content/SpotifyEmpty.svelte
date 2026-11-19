<script lang="ts">
  import { openSpotify, type MusicProvider } from '../island/spotify';
  import spotifyIcon from '../assets/spotify-icon.svg';
  import ExtensionIcon from "./ExtensionIcon.svelte";
  let { provider, error = "", onretry, onsearch }: { provider?: MusicProvider; error?: string; onretry?: () => void; onsearch?: () => void } = $props();
  const appleMusic = $derived(provider === "appleMusic");
  let busy=$state(false),message=$state('');
  async function open(){
    if(busy)return;busy=true;message='';
    try{if(!await openSpotify())message='Windows could not open Spotify. Check that the desktop app is installed.';else onretry?.();}
    catch(cause){message=typeof cause==='string'?cause:'Spotify could not open. Try opening the desktop app directly.';}
    finally{busy=false;}
  }
</script>

<section class="empty-player" aria-label={appleMusic ? "Apple Music connection status" : "Spotify connection status"}>
  {#if appleMusic}
    <span class="music-mark"><ExtensionIcon name="music" /></span>
    <h2>Apple Music</h2>
    <p>Start a song in the Apple Music desktop app.</p>
  {:else}
    <img src={spotifyIcon} width="28" height="28" alt="" />
    <h2>Spotify</h2>
    <p>Search Spotify here or play a song in Spotify or Apple Music.</p>
  {/if}
  {#if !appleMusic || onretry}
    <div class="actions">
      {#if !appleMusic}
        {#if onsearch}<button type="button" class="pill-btn" data-spotify-search-entry onclick={onsearch}>Search songs</button>{/if}
        <button type="button" class="secondary" disabled={busy} onclick={open}>{busy ? "Opening…" : "Open Spotify"}</button>
      {/if}
      {#if onretry}
        <button type="button" class="secondary" aria-label={appleMusic ? "Retry Apple Music connection" : "Retry Spotify connection"} onclick={() => { message = ""; onretry?.(); }}>Retry</button>
      {/if}
    </div>
  {/if}
  {#if error || (message && !appleMusic)}<p class="status" role="status">{(appleMusic ? "" : message) || error}</p>{/if}
</section>

<style>
  .empty-player{height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;padding:20px 66px 22px 28px;text-align:center;}
  img { opacity: .9; flex: none; }
  .music-mark { display: grid; place-items: center; width: 28px; height: 28px; }
  .music-mark :global(svg) { width: 24px; height: 24px; }
  h2 { margin: 0; font: 600 20px/25px var(--font-sans); letter-spacing: var(--tracking-tight); }
  p { margin: 0; color: var(--text-secondary); font: var(--type-caption); line-height: 17px; }
  .actions { display: flex; align-items: center; justify-content: center; flex-wrap: wrap; gap: 4px; margin-top: 3px; }
  .pill-btn { min-height: 30px; padding: 6px 12px; font: var(--type-label); }
  .secondary { min-height: 30px; border: 0; border-radius: 8px; background: transparent; color: var(--text-primary); font: var(--type-caption); cursor: pointer; padding: 4px 8px; }
  .secondary:hover { background: var(--surface-control-hover); }
  .secondary:disabled { opacity: .55; cursor: default; }
  .status { max-height: 34px; overflow-y: auto; overflow-wrap: anywhere; scrollbar-width: thin; }
</style>
