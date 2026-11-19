import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import Spotify from './Spotify.svelte';
import SpotifyEmpty from './SpotifyEmpty.svelte';
import SpotifySearch from './SpotifySearch.svelte';
import type { SpotifyTrack } from '../island/spotify';

const track:SpotifyTrack={key:'current',title:'Current song',artist:'Current artist',artwork:'https://i.scdn.co/image/current',playing:true,canToggle:true,canNext:true,canPrevious:true};
describe('Spotify content modes',()=>{
  it('makes search available in the empty player and renders it without an active track',()=>{
    const empty = render(SpotifyEmpty, { props: { onsearch: () => {} } }).body;
    expect(empty).toContain('data-spotify-search-entry');
    expect(empty).toContain('Search songs');
    expect(empty).toContain('Open Spotify');

    const html = render(SpotifySearch, { props: { trackKey: null, onclose: () => {} } }).body;
    expect(html).toContain('aria-label="Song search"');
    expect(html).toContain('Back to player');
    expect(html).not.toContain('aria-label="Playback controls"');
    expect(html).not.toContain('Current song');
    expect(html).not.toContain('Show lyrics');
    expect(html).not.toContain('LRCLIB');
  });
  it('keeps an accessible pin surface behind the controls (the cover itself never pins)',()=>{
    const html=render(Spotify,{props:{track,oncontrol:async()=>true,artworkVideo:true,onpin:()=>{}}}).body;
    expect(html).toContain('aria-label="Keep Spotify open"');
    expect(html).toMatch(/class="pin-surface(?:\s|")/);
    expect(html).not.toMatch(/class="artwork-button[^"]*"[^>]*aria-pressed/);
    expect(html).toContain('aria-label="Playback controls"');
  });
});
