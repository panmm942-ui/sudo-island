import { invoke } from '@tauri-apps/api/core';
import { emitTo, listen } from '@tauri-apps/api/event';
import { validArtworkUrl } from './artwork';
import { validCanvasUrl } from './canvas';
import type { VideoQuality } from './videoQuality';
import type { VideoSyncClock } from './videoSync';

export type VideoViewMode = 'fullscreen' | 'desktop';
export type VideoPresentation = 'video' | 'artwork' | 'lyrics';
export type VideoViewInput = { url: string | null; quality: VideoQuality; playing: boolean; pauseWithSong: boolean; presentation?: VideoPresentation; clock?: VideoSyncClock; expectedGeneration?:number };
export type VideoViewState = VideoViewInput & { mode: VideoViewMode; generation?:number };
export type VideoView = VideoViewState;
export type VideoPlaybackFailure = { generation: number; url: string; key: string; quality: VideoQuality };

export function validVideoViewUrl(url: unknown): url is string {
  return validArtworkUrl(url) || validCanvasUrl(url);
}
export function openVideoView(input: VideoViewState): Promise<VideoViewState> {
  if (!validVideoInput(input)) return Promise.reject(new Error('This track has no supported video.'));
  return invoke('open_video_view', input);
}
export function syncVideoView(input: VideoViewInput): Promise<VideoViewState | null> {
  if (!validVideoInput(input)) return Promise.reject(new Error('This track has no supported video.'));
  return invoke('sync_video_view', input);
}
export function validVideoInput(input: VideoViewInput): boolean {
  return input.url === null ? input.presentation === 'artwork' || input.presentation === 'lyrics' : validVideoViewUrl(input.url);
}
export function getVideoView(): Promise<VideoViewState | null> { return invoke('get_video_view'); }
/** Builds the hidden fullscreen window ahead of time so opening it is instant. */
export function prepareVideoView(): Promise<void> { return invoke('prepare_video_view'); }
export function closeVideoView(): Promise<void> { return invoke('close_video_view'); }
export function listenVideoView(update: (state: VideoViewState | null) => void): Promise<() => void> {
  return listen<VideoViewState | null>('video-view-changed', event => update(event.payload));
}

export function matchesVideoPlaybackFailure(view: VideoViewState | null, failure: VideoPlaybackFailure | null): boolean {
  return !!view && !!failure && typeof view.generation === 'number' &&
    view.generation === failure.generation && view.url === failure.url && view.clock?.key === failure.key &&
    view.quality === failure.quality && validVideoViewUrl(failure.url);
}

export function reportVideoPlaybackFailure(view: VideoViewState): Promise<void> {
  const { generation, url, clock, quality } = view;
  if (typeof generation !== 'number' || !url || !clock?.key || !validVideoViewUrl(url)) return Promise.resolve();
  return emitTo('island', 'video-playback-failed', { generation, url, key: clock.key, quality });
}

export function listenVideoPlaybackFailure(update: (failure: VideoPlaybackFailure) => void): Promise<() => void> {
  return listen<VideoPlaybackFailure>('video-playback-failed', event => update(event.payload));
}
