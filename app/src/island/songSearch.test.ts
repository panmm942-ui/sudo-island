import { describe, expect, it, vi } from 'vitest';
import { playSearchResult, searchSongs, type SongResult } from './songSearch';

const result: SongResult = { id: '3OHfY25tqY28d16oZczHc8', uri: 'spotify:track:3OHfY25tqY28d16oZczHc8',
  title: 'One', artist: 'Artist', album: 'Album', artwork: 'https://i.scdn.co/image/abc', duration: 180, source: 'spotify' };
const deezer = { ...result, id: '1234567', uri: 'deezer:track:1234567', source: 'deezer' as const };

describe('native Spotify catalogue search', () => {
  it('uses the signed-in native Spotify catalogue and preserves playable identities', async () => {
    const call=vi.fn().mockResolvedValue([result]);
    expect(await searchSongs('  Song & artist  ',new AbortController().signal,call)).toEqual([result]);
    expect(call).toHaveBeenCalledExactlyOnceWith('spotify_search_tracks',{query:'Song & artist'});
  });
  it('stays idle on a short query or an already cancelled view', async () => {
    const call=vi.fn(); const controller=new AbortController();
    expect(await searchSongs(' x ',controller.signal,call)).toEqual([]);
    controller.abort(); await expect(searchSongs('Song',controller.signal,call)).rejects.toMatchObject({name:'AbortError'});
    expect(call).not.toHaveBeenCalled();
  });
  it('rejects an overlong query rather than silently changing the requested search', async () => {
    const call=vi.fn(); await expect(searchSongs('x'.repeat(201),new AbortController().signal,call)).rejects.toThrow(/200/);
    expect(call).not.toHaveBeenCalled();
  });
  it('cancels promptly and prevents a late old catalogue reply from becoming a result', async () => {
    let complete!:(value:SongResult[])=>void; const call=vi.fn(()=>new Promise<unknown>(resolve=>{complete=resolve as typeof complete;}));
    const old=new AbortController(); const pending=searchSongs('First',old.signal,call);
    old.abort(); await expect(pending).rejects.toMatchObject({name:'AbortError'});
    complete([result]); await Promise.resolve();
    const current=vi.fn().mockResolvedValue([{...result,title:'Second'}]);
    expect(await searchSongs('Second',new AbortController().signal,current)).toEqual([{...result,title:'Second'}]);
  });
  it('preserves actionable native connection and rate-limit errors without opening anything else', async () => {
    const call=vi.fn().mockRejectedValue('Spotify rate limited player controls. Try again in 42 seconds.');
    await expect(searchSongs('Song',new AbortController().signal,call)).rejects.toThrow(/42 seconds/);
    expect(call).toHaveBeenCalledOnce();
  });
  it('rejects malformed track identities and drops unsafe artwork before rendering', async () => {
    const call=vi.fn().mockResolvedValue([{...result,uri:'spotify:track:wrong'}, {...result,id:'4OHfY25tqY28d16oZczHc8',uri:'spotify:track:4OHfY25tqY28d16oZczHc8',artwork:'https://evil.test/pixel'}]);
    expect(await searchSongs('Song',new AbortController().signal,call)).toEqual([{...result,id:'4OHfY25tqY28d16oZczHc8',uri:'spotify:track:4OHfY25tqY28d16oZczHc8',artwork:null}]);
  });
  it('starts only the selected Spotify URI on the same native track and preserves rejection details', async () => {
    const call=vi.fn().mockResolvedValue('played');
    await expect(playSearchResult(result,'native-song-key',call)).resolves.toBe('played');
    expect(call).toHaveBeenCalledExactlyOnceWith('spotify_play_track',{uri:result.uri,trackKey:'native-song-key'});
    call.mockRejectedValue('Spotify Premium and playback permission are required.');
    await expect(playSearchResult(result,'native-song-key',call)).rejects.toThrow(/Premium/);
  });
  it('uses background playback without an active track', async () => {
    const call = vi.fn().mockResolvedValue('played');
    await expect(playSearchResult(result, null, call)).resolves.toBe('played');
    expect(call).toHaveBeenCalledExactlyOnceWith('spotify_play_track', { uri: result.uri, trackKey: null });
  });
  it('does not report playing after native rejection or send malformed selection requests', async () => {
    const call=vi.fn().mockResolvedValue(false);
    await expect(playSearchResult(result,'native-song-key',call)).rejects.toThrow(/confirm/);
    call.mockClear(); await expect(playSearchResult({...result,uri:'https://open.spotify.com/'},'native-song-key',call)).rejects.toThrow();
    expect(call).not.toHaveBeenCalled();
  });
  it('keeps Spotify results only, so every listed row has a Spotify playback URI', async () => {
    const call = vi.fn().mockResolvedValue([deezer, { ...deezer, source: 'spotify' }, result, result]);
    expect(await searchSongs('Song', new AbortController().signal, call)).toEqual([result]);
  });
  it('requires a confirmed background outcome', async () => {
    const call = vi.fn().mockResolvedValue('opened');
    await expect(playSearchResult(result, null, call)).rejects.toThrow(/confirm/);
    expect(call).toHaveBeenCalledExactlyOnceWith('spotify_play_track', { uri: result.uri, trackKey: null });
  });
});
