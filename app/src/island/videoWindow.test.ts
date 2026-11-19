import {describe,it,expect} from 'vitest';
import {matchesVideoPlaybackFailure,validVideoInput,type VideoViewState} from './videoWindow';
describe('fullscreen media inputs',()=>{
  const base={quality:'best' as const,playing:true,pauseWithSong:true};
  it('permits artwork and lyrics without a video',()=>{
    expect(validVideoInput({...base,url:null,presentation:'artwork'})).toBe(true);
    expect(validVideoInput({...base,url:null,presentation:'lyrics'})).toBe(true);
    expect(validVideoInput({...base,url:null,presentation:'video'})).toBe(false);
  });
  it('keeps source restrictions in every presentation',()=>{
    expect(validVideoInput({...base,url:'https://evil.test/movie.mp4',presentation:'lyrics'})).toBe(false);
    expect(validVideoInput({...base,url:'https://canvaz.scdn.co/x.mp4',presentation:'video'})).toBe(true);
  });
  it('accepts only failure events for the exact current external playback',()=>{
    const url='https://canvaz.scdn.co/x.mp4';
    const view:VideoViewState={...base,url,mode:'fullscreen',generation:7,clock:{key:'track-1',playing:true}};
    const failure={generation:7,url,key:'track-1',quality:'best' as const};
    expect(matchesVideoPlaybackFailure(view,failure)).toBe(true);
    expect(matchesVideoPlaybackFailure(view,{...failure,key:'track-2'})).toBe(false);
    expect(matchesVideoPlaybackFailure(view,{...failure,quality:'480'})).toBe(false);
    expect(matchesVideoPlaybackFailure(view,{...failure,generation:6})).toBe(false);
    expect(matchesVideoPlaybackFailure(view,{...failure,url:'https://canvaz.scdn.co/new.mp4'})).toBe(false);
  });
});
