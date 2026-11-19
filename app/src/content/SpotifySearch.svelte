<script module lang="ts">
  let rememberedSearchRetryAt = 0;
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import { playSearchResult, searchSongs, type SongResult } from "../island/songSearch";

  let { trackKey, onclose, onsetup }: { trackKey: string | null; onclose: () => void; onsetup?: () => void } = $props();
  let query = $state("");
  let error = $state("");
  let feedback = $state("");
  let selecting = $state<string | null>(null);
  let searching = $state(false);
  let results = $state.raw<SongResult[] | null>(null);
  let resultQuery = $state("");
  let resultFreshUntil = 0;
  let searchRetryAt = $state(rememberedSearchRetryAt);
  let now = $state(Date.now());
  const searchWait = $derived(Math.max(0, Math.ceil((searchRetryAt - now) / 1000)));
  let controller: AbortController | undefined;
  let mounted = true;

  onDestroy(() => {
    mounted = false;
    controller?.abort();
  });

  $effect(() => {
    const deadline = searchRetryAt;
    if (deadline <= Date.now()) return;
    const tick = setInterval(() => {
      now = Date.now();
      if (now >= deadline) clearInterval(tick);
    }, 1000);
    return () => clearInterval(tick);
  });

  function readableWait(seconds: number) {
    if (seconds >= 3600) {
      const minutes = Math.ceil(seconds / 60);
      const hours = Math.floor(minutes / 60);
      const rest = minutes % 60;
      return `${hours} ${hours === 1 ? "hour" : "hours"}${rest ? ` ${rest} min` : ""}`;
    }
    if (seconds >= 120) return `${Math.ceil(seconds / 60)} minutes`;
    return `${seconds} ${seconds === 1 ? "second" : "seconds"}`;
  }

  function readableError(message: string) {
    return message.replace(/(is rate limited\. Try again in )(\d{1,8}) seconds?/i,
      (_, prefix, seconds) => prefix + readableWait(Number(seconds)));
  }

  async function search(event: SubmitEvent) {
    event.preventDefault();
    const term = query.trim();
    if (term.length < 2 || searching || selecting || Date.now() < searchRetryAt) return;
    if (term === resultQuery && Date.now() < resultFreshUntil) return;

    const request = new AbortController();
    controller = request;
    const timeout = setTimeout(() => request.abort(), 18_000);
    searching = true;
    error = "";
    feedback = "";
    try {
      const found = await searchSongs(term, request.signal);
      if (mounted) {
        results = found;
        resultQuery = term;
        resultFreshUntil = Date.now() + 30_000;
      }
    } catch (cause) {
      if (mounted) {
        const message = cause instanceof Error ? cause.message : "Song search is unavailable.";
        const match = /^Spotify desktop search is rate limited\. Try again in (\d{1,8}) seconds?\.?$/i.exec(message);
        if (!request.signal.aborted && match) {
          now = Date.now();
          searchRetryAt = Math.max(searchRetryAt, now + Number(match[1]) * 1000);
          rememberedSearchRetryAt = searchRetryAt;
        } else {
          error = request.signal.aborted ? "Search timed out. Try again." : readableError(message);
        }
      }
    } finally {
      clearTimeout(timeout);
      if (mounted) searching = false;
    }
  }

  async function select(result: SongResult) {
    if (selecting || searching) return;
    selecting = result.uri;
    error = "";
    feedback = "";

    try {
      await playSearchResult(result, trackKey);
      if (mounted) feedback = `Playing ${result.title}`;
    } catch (cause) {
      if (mounted) error = cause instanceof Error ? readableError(cause.message) : "Spotify could not play that song.";
    } finally {
      if (mounted) selecting = null;
    }
  }

  function focusSearch(input: HTMLInputElement) {
    input.focus();
  }

  function durationLabel(seconds: number) {
    return `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;
  }
</script>

<form class="search-form" aria-label="Song search" onsubmit={search} onwheel={(event) => event.stopPropagation()}>
  <div class="search-header">
    <label for="spotify-search-input">Search Spotify</label>
    <button type="button" class="search-back" aria-label="Back to player" onclick={onclose}>Back</button>
  </div>
  <div class="search-input">
    <input id="spotify-search-input" aria-label="Search songs" use:focusSearch bind:value={query}
      type="search" maxlength="200" placeholder="Song or artist" autocomplete="off" />
    <button type="submit" disabled={query.trim().length < 2 || searching || !!selecting || searchWait > 0}>Search</button>
  </div>

  {#if searchWait > 0}
    <p class="feedback" role="alert">Spotify search is rate limited. Try again in {readableWait(searchWait)}.</p>
  {/if}
  {#if error}
    <div class="error">
      <p class="feedback" role="alert">{error}</p>
      {#if onsetup && error.includes("Settings → Music")}<button type="button" onclick={onsetup}>Music settings</button>{/if}
    </div>
  {/if}
  {#if feedback}<p class="feedback" role="status">{feedback}</p>{/if}
  {#if searching}<p role="status">Finding songs…</p>{/if}
  {#if results}
    {#if results.length}
      <div class="results" aria-label="Song results">
        {#if resultQuery !== query.trim()}<p class="hint">Results for {resultQuery}</p>{/if}
        {#each results as result (result.uri)}
          <button type="button" class="result" aria-label={`Play ${result.title} by ${result.artist}`}
            disabled={searching || selecting !== null} aria-busy={selecting === result.uri} onclick={() => select(result)}>
            {#if result.artwork}
              <img class="artwork" src={result.artwork} width="32" height="32" alt="" loading="lazy" />
            {:else}
              <span class="artwork placeholder" aria-hidden="true">♪</span>
            {/if}
            <span class="result-copy">
              <strong title={result.title}>{result.title}</strong>
              <small title={result.artist}>{result.artist}</small>
            </span>
            <span class="duration tabular" aria-hidden="true">{selecting === result.uri ? "…" : durationLabel(result.duration)}</span>
          </button>
        {/each}
      </div>
    {:else if !searching && !error && searchWait === 0 && resultQuery === query.trim()}
      <p role="status">No matches. Try a different title or artist.</p>
    {/if}
  {:else if !searching && !error && searchWait === 0}
    <p class="hint">Keep Spotify running. Choose a song here to play it in the background.</p>
  {/if}
</form>

<style>
  .search-form {
    position: absolute;
    inset: 16px 62px 16px 58px;
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-height: 0;
    text-align: left;
  }
  .search-header, .search-input { display: flex; align-items: center; gap: 8px; flex: none; }
  .search-header { justify-content: space-between; }
  .search-header label { font: var(--type-title); }
  input {
    min-width: 0;
    height: 32px;
    flex: 1;
    padding: 0 8px;
    border: 1px solid var(--indicator-idle);
    border-radius: 8px;
    background: var(--surface-control);
    color: var(--text-primary);
    font: var(--type-body);
  }
  button {
    height: 32px;
    flex: none;
    border: 0;
    border-radius: 8px;
    padding: 0 10px;
    background: var(--surface-control);
    color: inherit;
    font: var(--type-label);
    cursor: pointer;
  }
  button:hover { background: var(--surface-control-hover); }
  button:disabled { opacity: .58; cursor: default; }
  .search-back { height: 28px; background: transparent; font: var(--type-caption); }
  p { margin: 0; font: var(--type-caption); }
  .feedback { max-height: 42px; overflow-y: auto; }
  .error { display: flex; gap: 8px; align-items: center; min-height: 0; }
  .error button { min-height: 24px; height: auto; }
  .hint, .duration, small { color: var(--text-secondary); }
  .results {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 3px;
    scrollbar-width: thin;
  }
  .result {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: auto;
    min-height: 44px;
    padding: 6px;
    background: transparent;
    text-align: left;
  }
  .result:focus-visible { outline-offset: -2px; }
  .artwork { display: block; flex: none; width: 32px; height: 32px; object-fit: cover; border-radius: 6px; }
  .placeholder { display: grid; place-items: center; background: var(--surface-control); }
  .result-copy { min-width: 0; flex: 1; }
  strong, small { display: block; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  strong { font: var(--type-label); }
  small, .duration { font: var(--type-caption); }
</style>
