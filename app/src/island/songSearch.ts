import { invoke } from '@tauri-apps/api/core';

export type SongResult = { id:string; uri:string; title:string; artist:string; album:string; artwork:string|null; duration:number; source:'spotify' };
type NativeCall = (command:string,args?:Record<string,unknown>)=>Promise<unknown>;
const validSpotifyId=(value:unknown):value is string=>typeof value==='string'&&/^[a-zA-Z0-9]{22}$/.test(value);
function validSelection(row:Record<string,unknown>):boolean {
  return row.source==='spotify'&&validSpotifyId(row.id)&&row.uri===`spotify:track:${row.id}`;
}
function artwork(value:unknown):string|null {
  if(typeof value!=='string'||value.length>2048)return null;
  try {
    const url=new URL(value);
    if(url.protocol!=='https:'||url.port||url.username||url.password||url.hash)return null;
    return url.hostname==='i.scdn.co'&&url.pathname.startsWith('/image/')?value:null;
  }
  catch{return null;}
}
const failure=(error:unknown)=>error instanceof Error?error:new Error(typeof error==='string'?error:'Song search is unavailable. Try again later.');

/** Only deliberate submits call the native Spotify catalogue search. Closing the view
 * aborts promptly and discards any already-running native HTTP reply.
 * Credentials, response bodies and account state remain in the native layer.
 */
export async function searchSongs(query:string,signal:AbortSignal,call:NativeCall=invoke):Promise<SongResult[]> {
  signal.throwIfAborted();
  const term=query.trim();
  if(term.length<2)return [];
  if(term.length>200)throw new Error('Search with up to 200 characters.');
  const data=await new Promise<unknown>((resolve,reject)=>{
    const abort=()=>reject(signal.reason??new DOMException('Search cancelled.','AbortError'));
    signal.addEventListener('abort',abort,{once:true});
    void call('spotify_search_tracks',{query:term}).then(resolve,error=>reject(failure(error)))
      .finally(()=>signal.removeEventListener('abort',abort));
    if(signal.aborted)abort();
  });
  signal.throwIfAborted();
  if(!Array.isArray(data))throw new Error('Song search returned an unreadable response.');
  const seen=new Set<string>();
  return data.slice(0,10).flatMap((row:Record<string,unknown>):SongResult[]=>{
    if(!row||!validSelection(row)||seen.has(String(row.uri))||
      typeof row.title!=='string'||!row.title.trim()||typeof row.artist!=='string'||!row.artist.trim()||
      typeof row.album!=='string'||typeof row.duration!=='number'||!Number.isFinite(row.duration)||row.duration<=0)return [];
    seen.add(String(row.uri));
    return [{id:String(row.id),uri:String(row.uri),title:row.title.slice(0,500),artist:row.artist.slice(0,500),
      album:row.album.slice(0,500),artwork:artwork(row.artwork),duration:row.duration,source:'spotify'}];
  });
}

export async function playSearchResult(result:SongResult,trackKey:string|null,call:NativeCall=invoke):Promise<'played'> {
  if(!validSelection(result)||(trackKey!==null&&(!trackKey||trackKey.length>2000)))throw new Error('This song selection is unavailable. Search again.');
  let outcome:unknown;
  try{outcome=await call('spotify_play_track',{uri:result.uri,trackKey});}
  catch(error){throw failure(error);}
  if(outcome!=='played')throw new Error('Spotify did not confirm this selection. Try again.');
  return outcome;
}
