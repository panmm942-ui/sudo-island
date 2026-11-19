import { expect, test } from "@playwright/test";
import type { SongResult } from "../../src/island/songSearch";
import type { SpotifyTrack } from "../../src/island/spotify";

type PlaybackRequest = { uri: string; trackKey: string | null };
type Fixture = {
  track: SpotifyTrack | null;
  lastReadKey: string | null;
  requests: PlaybackRequest[];
  searches: string[];
  searchError: string | null;
};

declare global {
  interface Window { spotifySearchFixture: Fixture; }
}

const result: SongResult = {
  id: "3OHfY25tqY28d16oZczHc8",
  uri: "spotify:track:3OHfY25tqY28d16oZczHc8",
  title: "Blue Hour",
  artist: "Sudo Sessions",
  album: "Evening Studies",
  artwork: null,
  duration: 214,
  source: "spotify",
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript((song) => {
    const fixture: Fixture = { track: null, lastReadKey: null, requests: [], searches: [], searchError: null };
    let callbackId = 0;
    Object.assign(window, {
      isTauri: true,
      spotifySearchFixture: fixture,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "island" }, currentWebview: { label: "island", windowLabel: "island" } },
        transformCallback: () => ++callbackId,
        unregisterCallback() {},
        convertFileSrc: (path: string) => path,
        async invoke(command: string, args?: PlaybackRequest | { query: string }) {
          if (command === "spotify_state") {
            fixture.lastReadKey = fixture.track?.key ?? null;
            return fixture.track;
          }
          if (command === "spotify_search_tracks" && args && "query" in args) {
            fixture.searches.push(args.query);
            if (fixture.searchError) return Promise.reject(fixture.searchError);
            return [song];
          }
          if (command === "spotify_play_track" && args && "uri" in args) {
            fixture.requests.push(args);
            return "played";
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
      videoEnabled: false,
      edgeAvoidance: false,
    }));
    localStorage.setItem("sudo.island.page", "spotify");
  }, result);
  await page.route(/^https?:\/\/(?!127\.0\.0\.1:)/, (route) => route.abort());
  await page.goto("/?state=expanded");
});

test("Spotify search works before playback and keeps results when a track appears", async ({ page }) => {
  test.setTimeout(35_000);
  const panel = page.locator(".content");
  await expect(panel.getByRole("button", { name: "Quick navigation", exact: true })).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Keep open", exact: true })).toHaveCount(0);
  await panel.getByRole("button", { name: "Search songs" }).click();
  const form = panel.getByRole("form", { name: "Song search" });
  await expect(form.getByRole("searchbox")).toBeFocused();
  await form.getByRole("searchbox").fill("Blue Hour");
  await form.getByRole("searchbox").press("Enter");
  await form.getByRole("button", { name: "Play Blue Hour by Sudo Sessions" }).click();
  await expect(form.getByRole("status")).toHaveText("Playing Blue Hour");
  await page.mouse.move(5, 275);
  await expect(form).toBeVisible();

  const track: SpotifyTrack = {
    key: "now-playing",
    title: "Blue Hour",
    artist: "Sudo Sessions",
    artwork: null,
    playing: true,
    canToggle: true,
    canNext: true,
    canPrevious: true,
  };
  await page.evaluate((nextTrack) => {
    window.spotifySearchFixture.track = nextTrack;
  }, track);
  await expect.poll(() => page.evaluate(() => window.spotifySearchFixture.lastReadKey)).toBe(track.key);
  await form.getByRole("button", { name: "Play Blue Hour by Sudo Sessions" }).click();
  await expect(form.getByRole("status")).toHaveText("Playing Blue Hour");
  expect(await page.evaluate(() => window.spotifySearchFixture.requests))
    .toEqual([{ uri: result.uri, trackKey: null }, { uri: result.uri, trackKey: track.key }]);
  await page.keyboard.press("Escape");
  await expect(form).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Search Spotify", exact: true })).toBeFocused();
  await panel.getByRole("button", { name: "Show lyrics" }).click();
  await expect(panel.getByRole("button", { name: "Close lyrics", exact: true })).toBeVisible();
});

test("Spotify search cooldown preserves playable results and blocks duplicate searches", async ({ page }) => {
  await page.getByRole("button", { name: "Search songs" }).click();
  const form = page.getByRole("form", { name: "Song search" });
  const search = form.getByRole("searchbox");
  const submit = form.getByRole("button", { name: "Search", exact: true });
  const priorResult = form.getByRole("button", { name: "Play Blue Hour by Sudo Sessions" });

  await search.fill("Blue Hour");
  await search.press("Enter");
  await expect(priorResult).toBeEnabled();
  await page.evaluate(() => {
    window.spotifySearchFixture.searchError = "Spotify desktop search is rate limited. Try again in 65 seconds.";
  });

  await search.fill("Other recording");
  await search.press("Enter");
  await expect(form.getByRole("alert")).toHaveText(/Spotify search is rate limited\. Try again in \d+ seconds\./);
  await expect(submit).toBeDisabled();
  await expect(priorResult).toBeEnabled();

  await search.press("Enter");
  await form.evaluate((element) => (element as HTMLFormElement).requestSubmit());
  expect(await page.evaluate(() => window.spotifySearchFixture.searches)).toEqual(["Blue Hour", "Other recording"]);

  await priorResult.click();
  await expect(form.getByText("Playing Blue Hour", { exact: true })).toBeVisible();
  await expect(submit).toBeDisabled();
  expect(await page.evaluate(() => window.spotifySearchFixture.requests))
    .toEqual([{ uri: result.uri, trackKey: null }]);
});
