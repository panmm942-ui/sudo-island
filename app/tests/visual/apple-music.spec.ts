import { expect, test } from "@playwright/test";
import type { SpotifyTrack } from "../../src/island/spotify";

type AppleFixture = {
  track: SpotifyTrack;
  requests: { command: string; args?: Record<string, unknown> }[];
};

declare global {
  interface Window { appleMusicFixture: AppleFixture; }
}

test("Apple Music gets the shared player, Windows transport and only Apple motion artwork", async ({ page }) => {
  test.setTimeout(30_000);
  const track: SpotifyTrack = {
    key: 'appleMusic:["Apple Fixture","Fixture Artist","Fixture Album"]',
    provider: "appleMusic",
    title: "Apple Fixture",
    artist: "Fixture Artist",
    album: "Fixture Album",
    artwork: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jA1wAAAAASUVORK5CYII=",
    duration: 180,
    position: 30,
    playing: true,
    canToggle: true,
    canNext: true,
    canPrevious: true,
    canSeek: true,
  };
  await page.addInitScript((song) => {
    const fixture: AppleFixture = { track: song, requests: [] };
    let callbackId = 0;
    Object.assign(window, {
      isTauri: true,
      appleMusicFixture: fixture,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "island" }, currentWebview: { label: "island", windowLabel: "island" } },
        transformCallback: () => ++callbackId,
        unregisterCallback() {},
        convertFileSrc: (path: string) => path,
        async invoke(command: string, args?: Record<string, unknown>) {
          fixture.requests.push({ command, args });
          if (command === "spotify_state") return { ...fixture.track };
          if (command === "spotify_control") {
            if (args?.action === "toggle") fixture.track.playing = !fixture.track.playing;
            return true;
          }
          if (command === "spotify_seek") {
            fixture.track.position = Number(args?.seconds);
            return true;
          }
          if (command === "plugin:window|scale_factor") return 1;
          if (command.startsWith("plugin:event|")) return ++callbackId;
          return null;
        },
      },
    });
    localStorage.setItem("sudo.preferences.v1", JSON.stringify({
      rememberPage: true,
      checkUpdates: false,
      edgeAvoidance: false,
      spotifySeekBar: true,
      spotifyShowRepeat: true,
      spotifyShowShuffle: true,
      spotifyBackground: "canvas",
      video: true,
      videoCanvasFallback: true,
    }));
    localStorage.setItem("sudo.island.page", "spotify");
  }, track);
  await page.route(/^https?:\/\/(?!127\.0\.0\.1:)/, (route) => route.abort());
  await page.goto("/?state=expanded");
  const panel = page.locator(".content");
  const player = panel.getByRole("region", { name: "Apple Music now playing" });
  await expect(player.getByText(track.title)).toBeVisible();
  await expect(player.getByText(track.artist)).toBeVisible();
  await expect(player.locator("img.artwork")).toHaveAttribute("src", track.artwork!);
  await expect(panel.getByRole("button", { name: "Apple Music", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(player.getByRole("button", { name: "Search Spotify", exact: true })).toHaveCount(0);
  await expect(player.getByRole("button", { name: /shuffle|repeat/i })).toHaveCount(0);

  await player.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(player.getByRole("button", { name: "Play", exact: true })).toBeVisible();
  await player.getByRole("button", { name: "Next track", exact: true }).click();
  await player.getByRole("slider", { name: "Song position" }).fill("42");
  const calls = () => page.evaluate(() => window.appleMusicFixture.requests);
  await expect.poll(calls).toEqual(expect.arrayContaining([
    { command: "spotify_control", args: { action: "toggle", trackKey: track.key } },
    { command: "spotify_control", args: { action: "next", trackKey: track.key } },
    { command: "spotify_seek", args: { seconds: 42, trackKey: track.key } },
    { command: "artwork_lookup", args: { track: { key: track.key, title: track.title, artist: track.artist, album: track.album, duration: track.duration } } },
  ]));
  await page.evaluate(() => { window.appleMusicFixture.track.canSeek = false; });
  await expect(player.getByRole("slider", { name: "Song position" })).toHaveCount(0);
  await expect(player.getByRole("progressbar", { name: "Song progress" })).toBeVisible();

  await player.getByRole("button", { name: "Show lyrics" }).click();
  await expect(panel.getByRole("region", { name: "Song lyrics" })).toBeVisible();
  expect((await calls()).map(({ command }) => command)).not.toEqual(expect.arrayContaining(["spotify_cover_art"]));
  for (const command of ["canvas_lookup", "spotify_bridge_snapshot", "vocal_flush"]) {
    expect((await calls()).some((request) => request.command === command)).toBe(false);
  }
});
