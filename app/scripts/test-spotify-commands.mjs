import assert from 'node:assert/strict';
import vm from 'node:vm';
import fs from 'node:fs/promises';

const pairingKey = 'a'.repeat(64);
const token = 'fixture-spotify-token';
const uri = 'spotify:track:3OHfY25tqY28d16oZczHc8';
const start = Math.floor(Date.now() / 1000) * 1000;
const source = (await fs.readFile(new URL('../extensions/sudo-spotify-bridge.js', import.meta.url), 'utf8'))
  .replace('__SUDO_BRIDGE_KEY__', pairingKey);
const song = {
  uri, id: uri.slice(14), type: 'track', name: 'Blue Hour', artists: [{ name: 'Fixture' }],
  album: { name: 'Fixture album', images: [] }, duration_ms: 160000, extra: token,
};

async function flush() {
  for (let pass = 0; pass < 30; pass++) await Promise.resolve();
}

function setup({ search = async () => [song], storage = new Map(), now = start } = {}) {
  const requests = [];
  const writes = [];
  const timers = [];
  const listeners = new Map();
  let playing = null;
  const context = {
    AbortSignal, TextEncoder, URL, Date: { now: () => now, parse: Date.parse },
    setTimeout(callback, delay) { timers.push({ callback, delay }); return callback; },
    clearTimeout() {},
    setInterval(callback) { return callback; },
    clearInterval() {},
    localStorage: { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    __sudoSpotifyClient: {
      Player: {
        get data() { return playing ? { item: { uri: playing } } : null; },
        isPlaying: () => !!playing,
        playUri: async (track) => { writes.push(track); },
        addEventListener: (event, callback) => listeners.set(event, callback),
        removeEventListener: (event) => listeners.delete(event),
      },
      Platform: { AuthorizationAPI: { getState: () => ({ token: {
        accessToken: token, accessTokenExpirationTimestampMs: now + 60000,
      } }) } },
      searchTracks: search,
    },
    fetch(url, options) {
      assert.equal(url, 'http://127.0.0.1:43893/v1/playback', 'Catalogue search uses only the installed desktop route.');
      assert.equal(options.headers.Authorization, `Bearer ${pairingKey}`);
      assert.ok(!options.body.includes(token));
      return new Promise((resolve, reject) => requests.push({
        body: JSON.parse(options.body),
        reply: (command) => resolve(command ? { ok: true, status: 200, json: async () => command } : { ok: true, status: 204 }),
        refusal: (status) => resolve({ ok: false, status }),
        reject,
      }));
    },
  };
  vm.runInNewContext(source, context);
  return {
    writes, timers, requests, storage,
    async poll() {
      await flush();
      const request = requests.shift();
      assert.ok(request, 'The next poll should start after fetch settles, without a renderer interval.');
      return request;
    },
    advance(ms) { now += ms; },
    command(id, fields) { return { id: id.repeat(64), expiresAt: now + 8000, ...fields }; },
    playbackChanged(track) { playing = track; listeners.get('onupdate')?.(); },
  };
}

{
  const bridge = setup({ search: async (query) => {
    assert.equal(query, 'Blue Hour');
    return [song, { uri: 'spotify:playlist:wrong', name: 'Unexpected item', type: 'playlist' }];
  } });
  const idle = await bridge.poll();
  assert.deepEqual(idle.body, { result: null, waitMs: 20000 });
  bridge.advance(20000);
  idle.reply();
  const command = bridge.command('1', { kind: 'play', uri });
  (await bridge.poll()).reply(command);
  await flush();
  assert.deepEqual(bridge.writes, [uri]);
  assert.equal(bridge.requests.length, 0, 'Accepting a request is not proof of playback.');
  bridge.playbackChanged(uri);

  const played = await bridge.poll();
  assert.deepEqual(played.body.result, { id: command.id, status: 'played' });
  played.reply(command);
  const duplicate = await bridge.poll();
  assert.equal(duplicate.body.result, null);
  duplicate.reply(bridge.command('2', { kind: 'play', uri, expiresAt: start - 1 }));
  const expired = await bridge.poll();
  assert.equal(expired.body.result, null);
  expired.reply(bridge.command('3', { kind: 'search', query: 'Blue Hour' }));

  const result = (await bridge.poll()).body.result;
  assert.equal(result.status, 'results');
  assert.equal(result.data.tracks.items.length, 1);
  assert.equal(result.data.tracks.items[0].uri, uri);
  assert.ok(!JSON.stringify(result).includes(token));
  assert.deepEqual(bridge.writes, [uri], 'Duplicate and expired commands never write.');
}

{
  let calls = 0;
  const bridge = setup({ search: async () => {
    calls++;
    throw { status: 429, response: { headers: { 'Retry-After': new Date(start + 65_000).toUTCString() } } };
  } });
  const search = bridge.command('4', { kind: 'search', query: 'Blue Hour' });
  (await bridge.poll()).reply(search);
  const limited = await bridge.poll();
  assert.deepEqual(limited.body.result, { id: search.id, status: 'rate_limited', retryAfterSeconds: 65 });
  assert.deepEqual([...bridge.storage], [['sudo.bridge.searchRetryAt', String(start + 65_000)]]);
  bridge.advance(20_000);
  limited.reply(bridge.command('5', { kind: 'search', query: 'Another query' }));
  const held = await bridge.poll();
  assert.equal(held.body.result.retryAfterSeconds, 45);
  assert.equal(calls, 1, 'Repeated queries respect the same catalogue deadline.');
  held.reply(bridge.command('6', { kind: 'play', uri }));
  await flush();
  assert.deepEqual(bridge.writes, [uri], 'Catalogue cooling still allows local playback.');
  bridge.playbackChanged(uri);
  assert.equal((await bridge.poll()).body.result.status, 'played');

  const restarted = setup({ now: start + 30_000, storage: bridge.storage, search: async () => { calls++; return []; } });
  (await restarted.poll()).reply(restarted.command('7', { kind: 'search', query: 'After restart' }));
  assert.equal((await restarted.poll()).body.result.retryAfterSeconds, 35);
  assert.equal(calls, 1);
}

{
  const bridge = setup();
  (await bridge.poll()).reject(new TypeError('Offline'));
  await flush();
  assert.equal(bridge.requests.length, 0, 'A network error waits before reconnecting.');
  bridge.timers.find(({ delay }) => delay === 1500).callback();
  const retry = await bridge.poll();
  assert.equal(retry.body.waitMs, 20000);
  retry.refusal(400);
  const legacy = await bridge.poll();
  assert.deepEqual(legacy.body, { result: null });
  legacy.reply();
  await flush();
  assert.equal(bridge.requests.length, 0, 'Immediate legacy idle responses do not create a hot loop.');
}

console.log('Spotify commands: asynchronous long poll, confirmed playback, safe retries, and catalogue cooldown passed.');
