import assert from 'node:assert/strict';
import vm from 'node:vm';
import fs from 'node:fs/promises';
const searchHash = 'b'.repeat(64);
const source=(await fs.readFile(new URL('../extensions/spotify-client-adapter.js',import.meta.url),'utf8'))
  .replace('__SUDO_SPOTIFY_SEARCH_HASH__', searchHash);
const state={timestamp:Date.now()-1000,item:{uri:'spotify:track:3OHfY25tqY28d16oZczHc8'},positionAsOfTimestamp:10000,duration:180000,isPaused:false,isBuffering:false};
let playerUpdate,selection;
const player={getState:()=>state,play:(target,context,options)=>{selection={target,context,options};},
  _events:{addListener:(name,callback)=>{assert.equal(name,'update');playerUpdate=callback;}}},authorization={getState:()=>({token:{accessToken:'fixture-token-never-exported'}})};
const playerSymbol=Symbol('PlayerAPI'),authSymbol=Symbol('AuthorizationAPI');
const values=new Map([[playerSymbol,player],[authSymbol,authorization]]);
const registry={_map:values,resolve:key=>values.get(key)};
let limited=false;
const platform={getRegistry:()=>registry,getGraphQLLoader:()=>async(definition,vars)=>{
  assert.equal(definition.name,'searchTracks');
  assert.equal(definition.sha256Hash,searchHash);
  assert.equal(vars.searchTerm,'Catalogue test');
  if(limited)return {errors:[{extensions:{code:'TOO_MANY_REQUESTS',http:{headers:{'retry-after':'45'}}}}]};
  return {data:{searchV2:{tracksV2:{items:[{item:{data:{
    __typename:'Track',uri:state.item.uri,name:'Catalogue test',playability:{playable:true},
    artists:{items:[{profile:{name:'Fixture Artist'}}]},duration:{totalMilliseconds:180000},
    albumOfTrack:{name:'Fixture Album',coverArt:{sources:[]}},
  }}}]}}}};
}};
const fiber={stateNode:{current:{memoizedProps:{platform},child:null,sibling:null}}};
const root={'__reactContainer$test':fiber};
const timers=[];
const context={Date,Map,Set,Symbol,Object,Math,Promise,queueMicrotask,console,document:{querySelectorAll:()=>[root]},setInterval:fn=>(timers.push(fn),timers.length),clearInterval:()=>{}};
vm.runInNewContext(source,context);
assert.ok(context.__sudoSpotifyClient,'Direct reader must work with no Spicetify global');
assert.equal(context.Spicetify,undefined);
assert.equal(context.__sudoSpotifyClient.Player.data.item,state.item);
assert.ok(context.__sudoSpotifyClient.Player.getProgress()>=11000);
state.isPaused=true;assert.equal(context.__sudoSpotifyClient.Player.getProgress(),10000);
state.isPaused=false;state.isBuffering=true;assert.equal(context.__sudoSpotifyClient.Player.getProgress(),10000);
assert.equal(context.__sudoSpotifyClient.Platform.AuthorizationAPI,authorization);
assert.equal(context.__sudoSpotifyClient.Player.getDuration(),180000);
assert.equal(context.__sudoSpotifyClient.Player.canPlayUri(),true);
await context.__sudoSpotifyClient.Player.playUri(state.item.uri);
assert.deepEqual(JSON.parse(JSON.stringify(selection)),{target:{uri:state.item.uri},context:{},options:{}});
await assert.rejects(context.__sudoSpotifyClient.Player.playUri('spotify:playlist:invalid'));
const found=await context.__sudoSpotifyClient.searchTracks('Catalogue test',new AbortController().signal);
assert.equal(found[0].uri,state.item.uri);
assert.equal(found[0].artists[0].name,'Fixture Artist');
assert.equal(found[0].duration_ms,180000);
limited=true;
await assert.rejects(context.__sudoSpotifyClient.searchTracks('Catalogue test',new AbortController().signal),
  error=>error.status===429&&error.retryAfter==='45');
let changes=0;
context.__sudoSpotifyClient.Player.addEventListener('songchange',()=>changes++);
assert.equal(typeof playerUpdate,'function','Native player events must wake the bridge without waiting for background timers');
state.item={uri:'spotify:track:4OHfY25tqY28d16oZczHc8'};playerUpdate({data:state});await new Promise(queueMicrotask);
assert.equal(changes,1);
console.log('Standalone Spotify adapter: no Spicetify, direct registry, player clock, pause/buffering and authorization locality passed.');
