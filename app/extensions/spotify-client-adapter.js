/* Sudo's minimal current-player reader. No Spicetify, theme or CLI dependency.
 * Reads the existing React platform provider for playback and catalogue search.
 * It does not patch Spotify's bundles or export its authorization credentials.
 * Self-healing: Spotify can replace its player object while running (after sleep, a network
 * reconnect or an internal reload). The reader then finds the current one again, instead of
 * holding on to a stale player that no longer reports a song (which silenced Sudo for good).
 */
(function sudoSpotifyClient(){
  'use strict';
  if(globalThis.__sudoSpotifyClient)return;
  const listeners={songchange:new Set(),onplaypause:new Set(),onupdate:new Set()};
  const live={player:null,authorization:null,platform:null};
  const catalogueQueryHash='__SUDO_SPOTIFY_SEARCH_HASH__';
  let previousUri,previousPaused;
  function find(){
    const seen=new Set(),stack=[];
    for(const root of document.querySelectorAll('#root,#main')){
      const key=Object.keys(root).find(name=>name.startsWith('__reactContainer$')||name.startsWith('__reactFiber$'));
      if(key){const fiber=root[key];stack.push(fiber?.stateNode?.current??fiber);}
    }
    while(stack.length&&seen.size<5000){
      const fiber=stack.pop();if(!fiber||seen.has(fiber))continue;seen.add(fiber);
      const platform=fiber.memoizedProps?.platform;
      if(typeof platform?.getRegistry==='function'){
        const registry=platform.getRegistry();
        if(registry?._map instanceof Map&&typeof registry.resolve==='function'){
          const resolve=name=>{const key=[...registry._map.keys()].find(key=>typeof key==='symbol'&&key.description===name);return key?registry.resolve(key):null;};
          const player=resolve('PlayerAPI'),authorization=resolve('AuthorizationAPI');
          if(typeof player?.getState==='function'&&authorization)return {player,authorization,platform};
        }
      }
      // Visit the upper provider chain before the large sibling UI trees.
      stack.push(fiber.sibling,fiber.child);
    }
    return null;
  }
  function attach(found){
    if(found.player===live.player&&found.authorization===live.authorization&&found.platform===live.platform)return;
    live.player=found.player;live.authorization=found.authorization;
    live.platform=found.platform;
    const state=live.player.getState();previousUri=state?.item?.uri;previousPaused=state?.isPaused;
    if(typeof live.player._events?.addListener==='function')live.player._events.addListener('update',()=>{
      queueMicrotask(()=>{
        if(found.player!==live.player)return; // an older player we replaced
        const current=live.player.getState();
        const song=current?.item?.uri!==previousUri,paused=current?.isPaused!==previousPaused;
        previousUri=current?.item?.uri;previousPaused=current?.isPaused;
        if(song)for(const callback of listeners.songchange)callback();
        if(paused)for(const callback of listeners.onplaypause)callback();
        for(const callback of listeners.onupdate)callback();
      });
    });
  }
  const state=()=>{try{return live.player?.getState()??null;}catch{return null;}};
  const Player={
    get data(){return state();},
    getDuration:()=>Number(state()?.duration)||0,
    getProgress:()=>{const s=state();if(!s)return 0;const elapsed=s.isPaused||s.isBuffering?0:Math.max(0,Date.now()-s.timestamp);return Math.min(Number(s.duration)||0,Math.max(0,(Number(s.positionAsOfTimestamp)||0)+elapsed));},
    isPlaying:()=>!state()?.isPaused,
    canPlayUri:()=>typeof live.player?.play==='function',
    async playUri(uri){
      if(!/^spotify:track:[A-Za-z0-9]{22}$/.test(uri)||typeof live.player?.play!=='function')throw Error('Playback unavailable');
      return live.player.play({uri},{},{});
    },
    addEventListener:(name,callback)=>{if(typeof callback==='function')listeners[name]?.add(callback);},
    removeEventListener:(name,callback)=>listeners[name]?.delete(callback),
  };
  const Platform={get AuthorizationAPI(){return live.authorization;}};
  async function searchTracks(searchTerm,signal) {
    if(!/^[a-f0-9]{64}$/.test(catalogueQueryHash)||signal.aborted)return null;
    const load=live.platform?.getGraphQLLoader?.();
    if(typeof load!=='function')return null;
    // Use the installed client's own signed-in catalogue request and its bundled track query.
    const definition={name:'searchTracks',operation:'query',sha256Hash:catalogueQueryHash,value:null};
    const variables={searchTerm,offset:0,limit:10,numberOfTopResults:20,includePreReleases:false,
      includeAlbumPreReleases:false,includeAudiobooks:true,includeAuthors:false,includeEpisodeContentRatingsV2:false};
    const response=await load(definition,variables,undefined,undefined,signal);
    const limited=Array.isArray(response?.errors)&&response.errors.find(error=>{
      const details=error?.extensions;
      return Number(details?.http?.status??details?.status??details?.code)===429||
        ['RATE_LIMITED','TOO_MANY_REQUESTS','THROTTLED'].includes(details?.code);
    });
    if(limited)throw {status:429,retryAfter:limited.extensions?.retryAfter??limited.extensions?.http?.headers?.['retry-after']};
    const rows=response?.data?.searchV2?.tracksV2?.items;
    if(signal.aborted||response?.errors?.length||!Array.isArray(rows))return null;
    return rows.slice(0,10).flatMap(row=>{
      const track=row?.item?.data;
      if(track?.__typename!=='Track'||track.playability?.playable!==true||
        !/^spotify:track:[A-Za-z0-9]{22}$/.test(track.uri))return [];
      return [{type:'track',id:track.uri.slice(14),uri:track.uri,name:track.name,duration_ms:track.duration?.totalMilliseconds,
        artists:track.artists?.items?.map(artist=>({name:artist.profile?.name})),
        album:{name:track.albumOfTrack?.name,images:track.albumOfTrack?.coverArt?.sources}}];
    });
  }
  function discover(){const found=find();if(!found)return false;attach(found);globalThis.__sudoSpotifyClient??={Player,Platform,searchTracks};return true;}
  if(!discover()){const timer=setInterval(()=>{try{if(discover())clearInterval(timer);}catch{/* Wait for Spotify's provider to finish mounting. */}},1000);}
  // Every 5 s: a player that no longer reports a song may have been replaced; look again.
  setInterval(()=>{try{if(live.player&&!state()?.item)discover();}catch{/* Keep the current player. */}},5000);
})();
