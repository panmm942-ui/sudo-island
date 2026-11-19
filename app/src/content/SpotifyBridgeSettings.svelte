<script lang="ts">
  import {onMount} from 'svelte';
  import {invoke} from '@tauri-apps/api/core';
  let {enabled,onchange}:{enabled:boolean;onchange:(enabled:boolean)=>void}=$props();
  type Status={enabled:boolean;connected:boolean;commandsReady?:boolean;extensionPath:string|null;lastSeenAt:number|null;stale?:boolean};
  type Connector={installed:boolean;needsRepair:boolean;spotifyFound:boolean;lastSetupAt:number|null};
  let status=$state<Status|null>(null),busy=$state(false),error=$state(''),message=$state('');
  let connector=$state<Connector|null>(null),reading=false;
  async function read(){if(reading)return;reading=true;try{[status,connector]=await Promise.all([invoke<Status>('spotify_bridge_status'),invoke<Connector>('spotify_connector_status')]);}catch{/* Native-only integration. */}finally{reading=false;}}
  onMount(()=>{void read();const timer=setInterval(read,2000);return()=>clearInterval(timer);});
  async function toggle(next:boolean){busy=true;error='';try{status=await invoke<Status>('set_spotify_bridge_enabled',{enabled:next});onchange(status.enabled);}catch(cause){error=String(cause);}finally{busy=false;}}
  async function install(){busy=true;error='';message='';try{message=await invoke<string>('install_spotify_connector');status=await invoke<Status>('spotify_bridge_status');onchange(status.enabled);await read();}catch(cause){error=String(cause);}finally{busy=false;}}
  async function restore(){busy=true;error='';message='';try{message=await invoke<string>('restore_spotify_connector');status=await invoke<Status>('set_spotify_bridge_enabled',{enabled:false});onchange(false);await read();}catch(cause){error=String(cause);}finally{busy=false;}}
</script>
<div class="bridge">
  <label><span><strong>Connect to Spotify on this computer</strong><small>Search and play songs from the Island while Spotify runs in the background. The connection also provides original covers, Canvas and timed lyrics. Your Spotify token stays inside Spotify.</small></span><input type="checkbox" role="switch" aria-label="Local Spotify connection" checked={enabled} disabled={busy} onchange={event=>{const next=event.currentTarget.checked;event.currentTarget.checked=enabled;void toggle(next);}}/></label>
  <button type="button" class="pill-btn" disabled={busy} onclick={install}>{busy?'Preparing…':connector?.installed||status?.stale?'Repair Spotify connection':'Set up Spotify connection'}</button>
  {#if connector?.installed}<button type="button" class="pill-btn" disabled={busy} onclick={restore}>Restore previous Spotify setup</button>{/if}
  <p class="help">Adds only Sudo’s small connector and keeps a restore backup. Spotify restarts once. Uses the desktop download from spotify.com. After a Spotify update, use Repair if the connection stops.</p>
  {#if enabled}
    <p class="state" role="status">
      {#if status?.commandsReady}
        Spotify search and background playback are ready.
      {:else if status?.stale}
        Spotify is using an old pairing. Use Repair Spotify connection.
      {:else if connector?.needsRepair}
        The Spotify connection needs an update. Use Repair Spotify connection.
      {:else if status?.connected}
        Current song connected. Use Repair Spotify connection to enable catalogue playback.
      {:else if connector?.installed}
        Waiting for Spotify. Keep the desktop player running and signed in.
      {:else}
        Set up the connector to search and play songs from the Island.
      {/if}
    </p>
  {/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if error}<p class="error" role="status">{error}</p>{/if}
</div>
<style>
 .bridge{padding:12px 0;border-bottom:1px solid #ffffff15}.bridge label{display:flex;gap:16px;align-items:center;justify-content:space-between}.bridge strong{font:560 13px/18px var(--font-sans)}.bridge small,.help{display:block;font:var(--type-caption);opacity:.7;line-height:1.5;margin:5px 0}.bridge label span{min-width:0}.bridge input{flex:none;accent-color:var(--accent);width:18px;height:18px}.state{font:var(--type-caption);margin:10px 0}.pill-btn{margin:3px 0 6px}.error{font:var(--type-caption);white-space:normal;overflow-wrap:anywhere}
</style>
