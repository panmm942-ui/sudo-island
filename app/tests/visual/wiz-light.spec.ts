import { expect, test, type Locator } from "@playwright/test";

type LightState = {
  name: string;
  on: boolean;
  brightness: number;
  hue: number;
  saturation: number;
  colorTemperature: number;
};

type WiZState = LightState & {
  supportsBrightness: boolean;
  supportsColor: boolean;
  supportsTemperature: boolean;
  minBrightness: number;
  minColorTemperature: number;
  maxColorTemperature: number;
};

type Request = { command: string; args: unknown };
type SunsetStatus = {
  enabled: boolean;
  latitude: number | null;
  longitude: number | null;
  locationName: string | null;
  nextSunset: string | null;
  lastError: string | null;
  timeZone: string | null;
};

declare global {
  interface Window { lightRequests: Request[]; }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const requests: Request[] = [];
    const tapo: LightState = {
      name: "Desk lamp", on: true, brightness: 60, hue: 0, saturation: 0, colorTemperature: 2700,
    };
    const available = [
      { id: "aabbccddeeff", name: "Studio lamp", ip: "192.168.3.29", model: "WiZ Color" },
      { id: "aabbccddee10", name: "Hall lamp", ip: "192.168.3.30", model: "WiZ Dimmable" },
    ];
    const paired = new Map<string, { device: typeof available[number]; state: WiZState }>();
    const lan = new Map<string, {
      device: typeof available[number] & { provider: "hue" | "yeelight" };
      state: LightState;
    }>();
    const firstTapo = { id: "tapo", name: "Desk lamp", ip: "192.168.3.31", model: "L530E" };
    const tapoLamps = new Map<string, { device: typeof firstTapo; state: LightState }>();
    if (!new URLSearchParams(location.search).has("freshLights")) tapoLamps.set(firstTapo.id, { device: firstTapo, state: tapo });
    const disabledSunset: SunsetStatus = {
      enabled: false, latitude: null, longitude: null, locationName: null, nextSunset: null, lastError: null, timeZone: null,
    };
    let sunset = disabledSunset;
    let callbackId = 0;
    Object.assign(window, {
      isTauri: true,
      lightRequests: requests,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "island" }, currentWebview: { label: "island", windowLabel: "island" } },
        transformCallback: () => ++callbackId,
        unregisterCallback() {},
        convertFileSrc: (path: string) => path,
        async invoke(command: string, args?: {
          id?: string;
          ip?: string;
          name?: string;
          email?: string;
          password?: string;
          bridgeId?: string;
          lightId?: number;
          provider?: string;
          latitude?: number;
          longitude?: number;
          locationName?: string;
          update?: Partial<LightState>;
        }) {
          if (/^(wiz|tapo|hue|yeelight|lan_light|light_sunset)_/.test(command)) requests.push({ command, args });
          const light = args?.id ? paired.get(args.id) : undefined;
          switch (command) {
            case "tapo_list": return [...tapoLamps.values()].map(({ device }) => device);
            case "tapo_account": return tapoLamps.size ? "tapo@example.test" : null;
            case "tapo_discover": return tapoLamps.size
              ? [{ ip: "192.168.3.31", model: "L530E", paired: true }, { ip: "192.168.3.34", model: "L535E", paired: false }]
              : [{ ip: "192.168.3.31", model: "L530E", paired: false }];
            case "tapo_connect": {
              // The first lamp signs in. The next reuses the saved account and sends no credentials.
              const signedIn = args?.email === "tapo@example.test" && args.password === "fixture-password";
              const reused = tapoLamps.size > 0 && args?.email === undefined && args?.password === undefined;
              const second = args?.ip === "192.168.3.34";
              if ((args?.ip !== "192.168.3.31" && !second) || !(signedIn || reused)) {
                throw new Error("Unexpected Tapo test connection");
              }
              const result = second
                ? { device: { id: "tapo-242f29040506", name: "Bedside lamp", ip: "192.168.3.34", model: "L535E" }, state: { ...tapo, name: "Bedside lamp", on: false } }
                : { device: firstTapo, state: tapo };
              tapoLamps.set(result.device.id, result);
              return result;
            }
            case "tapo_state": return tapoLamps.get(args?.id ?? "")?.state;
            case "tapo_update": {
              const lamp = tapoLamps.get(args?.id ?? "");
              if (!lamp) throw new Error("Unknown saved Tapo lamp");
              return Object.assign(lamp.state, args?.update);
            }
            case "wiz_list": return [...paired.values()].map(({ device }) => device);
            case "wiz_discover": return available.slice(0, 1).map(({ ip, model }) => ({ ip, model }));
            case "wiz_connect": {
              const found = available.find((device) => device.ip === args?.ip);
              if (!found) throw new Error("Unknown test bulb");
              const device = { ...found, name: args?.name || found.name };
              const color = found === available[0];
              const state: WiZState = {
                ...tapo, name: device.name, on: true, brightness: 45,
                supportsBrightness: true, supportsColor: color, supportsTemperature: color,
                minBrightness: 10, minColorTemperature: color ? 2200 : 0, maxColorTemperature: color ? 6500 : 0,
              };
              const result = { device, state };
              paired.set(device.id, result);
              return result;
            }
            case "wiz_state": return light?.state;
            case "wiz_update": {
              if (!light) throw new Error("Unknown saved test bulb");
              if (args?.update?.hue !== undefined) light.state.colorTemperature = 0;
              return Object.assign(light.state, args?.update);
            }
            case "lan_light_list": return [...lan.values()].map(({ device }) => device);
            case "yeelight_discover": return [{ ip: "192.168.3.32", model: "Yeelight Color" }];
            case "yeelight_connect": {
              if (args?.ip !== "192.168.3.32") throw new Error("Unexpected Yeelight fixture address");
              const device = { id: "yeelight-0000000000000001", provider: "yeelight" as const, name: "Reading light", ip: args.ip, model: "color" };
              const result = { device, state: { ...tapo, name: device.name } };
              lan.set(device.id, result);
              return result;
            }
            case "hue_discover": return [{ ip: "192.168.3.33", model: "Philips Hue Bridge" }];
            case "hue_link_bridge": {
              if (args?.ip !== "192.168.3.33") throw new Error("Unexpected Hue fixture address");
              return {
                bridgeId: "001788fffe123456",
                lights: [{ id: 1, name: "Hall light", model: "LCT015" }, { id: 2, name: "Floor light", model: "LCT016" }],
              };
            }
            case "hue_connect": {
              if (args?.bridgeId !== "001788fffe123456" || args.lightId !== 2) throw new Error("Unexpected Hue fixture selection");
              const device = { id: "hue-fixture", provider: "hue" as const, name: "Floor light", ip: "192.168.3.33", model: "LCT016" };
              const result = { device, state: { ...tapo, name: device.name } };
              lan.set(device.id, result);
              return result;
            }
            case "lan_light_state": return lan.get(args?.id ?? "")?.state;
            case "lan_light_update": {
              const light = lan.get(args?.id ?? "");
              if (!light) throw new Error("Unknown saved LAN bulb");
              return Object.assign(light.state, args?.update);
            }
            case "light_sunset_status": return sunset;
            case "light_sunset_detect": return { latitude: 36.72, longitude: -4.42, label: "Windows location" };
            case "light_sunset_search": return [{ latitude: 41.38, longitude: 2.18, label: "Barcelona, Spain" }];
            case "light_sunset_save": {
              if (args?.id !== "tapo" || args.provider !== "tapo") throw new Error("Unexpected sunset fixture target");
              sunset = {
                enabled: true,
                latitude: args.latitude ?? null,
                longitude: args.longitude ?? null,
                locationName: args.locationName ?? null,
                nextSunset: "2027-01-10T17:20:00Z",
                lastError: null,
                timeZone: "Europe/Madrid",
              };
              return sunset;
            }
            case "light_sunset_disable": {
              if (args?.id !== "tapo" || args.provider !== "tapo") throw new Error("Unexpected sunset fixture target");
              sunset = disabledSunset;
              return sunset;
            }
            case "system_stats": return { cpu: 0, memory: 0, memoryUsedGb: 0, memoryTotalGb: 16, gpu: null };
            case "plugin:window|scale_factor": return 1;
            default: return command.startsWith("plugin:event|") ? ++callbackId : null;
          }
        },
      },
    });
    localStorage.setItem("sudo.preferences.v1", JSON.stringify({
      rememberPage: true,
      checkUpdates: false,
      videoEnabled: false,
      edgeAvoidance: false,
    }));
    localStorage.setItem("sudo.island.page", "tools");
    localStorage.setItem("sudo.tools.section", "light");
  });
  await page.route(/^https?:\/\/(?!127\.0\.0\.1:)/, (route) => route.abort());
});

test("schedules the selected bulb at sunset only after explicit enable", async ({ page }) => {
  test.setTimeout(35_000);
  await page.goto("/?state=expanded");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  await expect(tools.getByText("Desk lamp", { exact: true })).toBeVisible();
  const open = tools.getByRole("button", { name: "Sunset schedule", exact: true });
  await expect(open).toBeInViewport();
  await open.click();

  const schedule = tools.getByRole("region", { name: "Sunset schedule", exact: true });
  const enable = schedule.getByRole("button", { name: "Enable schedule", exact: true });
  await expect(enable).toBeDisabled();
  let requests = await page.evaluate(() => window.lightRequests.filter(({ command }) => command.startsWith("light_sunset_")));
  expect(requests).toEqual([{ command: "light_sunset_status", args: { provider: "tapo", id: "tapo" } }]);
  await expect(schedule.getByRole("link", { name: "Sunrise-Sunset.org" })).toHaveAttribute("href", "https://sunrise-sunset.org/");
  await schedule.getByRole("button", { name: "Use Windows location", exact: true }).click();
  await expect(schedule.getByText("Selected · Windows location", { exact: true })).toBeVisible();
  requests = await page.evaluate(() => window.lightRequests.filter(({ command }) => command === "light_sunset_save"));
  expect(requests).toEqual([]);
  await enable.click();
  await expect(schedule.getByText("On at sunset · Windows location", { exact: true })).toBeVisible();
  await expect(schedule.getByText(/Next sunset ·/)).toBeVisible();

  await schedule.getByRole("button", { name: "Change location", exact: true }).click();
  await schedule.getByRole("textbox", { name: "City or town", exact: true }).fill("Barcelona");
  await schedule.getByRole("button", { name: "Search", exact: true }).click();
  const matches = schedule.getByLabel("Matches", { exact: true });
  await expect(matches).toHaveValue("");
  await matches.selectOption({ label: "Barcelona, Spain" });
  await schedule.getByRole("button", { name: "Save schedule", exact: true }).click();
  await expect(schedule.getByText("On at sunset · Barcelona, Spain", { exact: true })).toBeVisible();
  await schedule.getByRole("button", { name: "Disable schedule", exact: true }).click();
  await expect(enable).toBeDisabled();
  requests = await page.evaluate(() => window.lightRequests.filter(({ command }) =>
    command === "light_sunset_save" || command === "light_sunset_disable"));
  expect(requests).toEqual([
    { command: "light_sunset_save", args: { provider: "tapo", id: "tapo", latitude: 36.72, longitude: -4.42, locationName: "Windows location" } },
    { command: "light_sunset_save", args: { provider: "tapo", id: "tapo", latitude: 41.38, longitude: 2.18, locationName: "Barcelona, Spain" } },
    { command: "light_sunset_disable", args: { provider: "tapo", id: "tapo" } },
  ]);
});

function savedLight(tools: Locator, name: string): Locator {
  return tools.getByRole("radiogroup", { name: "Saved lights", exact: true }).getByRole("radio", { name, exact: true });
}

test("adds a second Tapo lamp with the saved TP-Link account", async ({ page }) => {
  test.setTimeout(35_000);
  await page.goto("/?state=expanded");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  await expect(savedLight(tools, "Desk lamp")).toHaveAttribute("aria-checked", "true");
  await tools.getByRole("button", { name: "Add light", exact: true }).click();
  const tapo = tools.getByRole("button", { name: "Tapo", exact: true });
  await expect(tapo).toBeEnabled();
  await tapo.click();
  await tools.getByRole("button", { name: "Find bulbs", exact: true }).click();

  // The lamp that is already saved is marked, and the new one is chosen.
  const found = tools.getByRole("radiogroup", { name: "Found bulbs", exact: true });
  await expect(found.getByRole("radio", { name: /L530E.*Added/ })).toHaveAttribute("aria-checked", "false");
  await expect(found.getByRole("radio", { name: /L535E/ })).toHaveAttribute("aria-checked", "true");
  await expect(tools.getByText("tapo@example.test", { exact: true })).toBeVisible();
  await expect(tools.getByLabel("Password", { exact: true })).toHaveCount(0);
  await tools.getByRole("button", { name: "Pair light", exact: true }).click();

  await expect(savedLight(tools, "Bedside lamp")).toHaveAttribute("aria-checked", "true");
  await expect(savedLight(tools, "Desk lamp")).toHaveAttribute("aria-checked", "false");
  await tools.getByRole("button", { name: "Turn on", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Turn off", exact: true })).toBeVisible();
  await savedLight(tools, "Desk lamp").click();
  await tools.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Turn on", exact: true })).toBeVisible();

  const requests = await page.evaluate(() => window.lightRequests);
  expect(requests).toContainEqual({ command: "tapo_connect", args: { ip: "192.168.3.34" } });
  expect(requests.filter(({ command }) => command === "tapo_update")).toEqual([
    { command: "tapo_update", args: { id: "tapo-242f29040506", update: { on: true } } },
    { command: "tapo_update", args: { id: "tapo", update: { on: false } } },
  ]);
  expect(requests.some(({ command }) => command === "tapo_disconnect")).toBe(false);
});

async function expectBrandPicker(tools: Locator): Promise<void> {
  await expect(tools.getByText("Choose your light brand", { exact: true })).toBeVisible();
  await expect(tools.getByRole("button", { name: "Tapo", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(tools.getByRole("button", { name: "WiZ", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(tools.getByRole("button", { name: "Xiaomi / Yeelight", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(tools.getByRole("button", { name: "Philips Hue", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(tools.getByRole("button", { name: "Find bulbs", exact: true })).toHaveCount(0);
  await expect(tools.getByRole("button", { name: "Find bridges", exact: true })).toHaveCount(0);
  await expect(tools.getByRole("button", { name: "Enter IP manually", exact: true })).toHaveCount(0);
}

test("chooses the brand before setting up the first light", async ({ page }) => {
  await page.goto("/?state=expanded&freshLights=1");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  const tapo = tools.getByRole("button", { name: "Tapo", exact: true });
  const wiz = tools.getByRole("button", { name: "WiZ", exact: true });
  await expectBrandPicker(tools);
  await expect(tapo).toBeEnabled();
  await expect(wiz).toBeEnabled();
  const beforeChoice = await page.evaluate(() => [...new Set(window.lightRequests.map(({ command }) => command))]);
  expect(beforeChoice.sort()).toEqual(["lan_light_list", "tapo_list", "wiz_list"]);

  await wiz.click();
  await expect(wiz).toHaveAttribute("aria-pressed", "true");
  await tools.getByRole("button", { name: "Enter IP manually", exact: true }).click();
  await expect(tools.getByLabel("Name", { exact: true })).toBeVisible();
  await expect(tools.getByLabel("Tapo email", { exact: true })).toHaveCount(0);

  await tapo.click();
  await expect(tapo).toHaveAttribute("aria-pressed", "true");
  await tools.getByRole("button", { name: "Find bulbs", exact: true }).click();
  await tools.getByLabel("Tapo email", { exact: true }).fill("tapo@example.test");
  await tools.getByLabel("Password", { exact: true }).fill("fixture-password");
  await tools.getByRole("button", { name: "Pair light", exact: true }).click();
  await expect(tools.getByText("Desk lamp", { exact: true })).toBeVisible();

  const afterPairing = await page.evaluate(() => window.lightRequests.map(({ command }) => command));
  expect(afterPairing.filter((command) => !["lan_light_list", "tapo_list", "wiz_list"].includes(command)))
    .toEqual(["tapo_discover", "tapo_connect"]);
  await tools.getByRole("button", { name: "Add light", exact: true }).click();
  await expectBrandPicker(tools);
});

test("pairs a Xiaomi light and an explicitly selected Hue light, and resets Add light after cancel", async ({ page }) => {
  test.setTimeout(35_000);
  await page.goto("/?state=expanded&freshLights=1");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  await expectBrandPicker(tools);
  await tools.getByRole("button", { name: "Xiaomi / Yeelight", exact: true }).click();
  await tools.getByRole("button", { name: "Find bulbs", exact: true }).click();
  await tools.getByRole("button", { name: "Connect light", exact: true }).click();
  await expect(tools.getByText("Reading light", { exact: true })).toBeVisible();

  const add = tools.getByRole("button", { name: "Add light", exact: true });
  await add.click();
  await expectBrandPicker(tools);
  await tools.getByRole("button", { name: "WiZ", exact: true }).click();
  await tools.getByRole("button", { name: "Cancel", exact: true }).click();
  await add.click();
  await expectBrandPicker(tools);
  await tools.getByRole("button", { name: "Philips Hue", exact: true }).click();
  await tools.getByRole("button", { name: "Find bridges", exact: true }).click();
  await tools.getByRole("button", { name: "Connect bridge", exact: true }).click();
  const connectLight = tools.getByRole("button", { name: "Connect light", exact: true });
  await expect(connectLight).toBeDisabled();
  await tools.getByLabel("Hue light", { exact: true }).selectOption({ label: "Floor light" });
  await connectLight.click();
  await expect(savedLight(tools, "Floor light")).toHaveAttribute("aria-checked", "true");
  await expect(savedLight(tools, "Reading light")).toHaveAttribute("aria-checked", "false");
  await tools.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Turn on", exact: true })).toBeVisible();
  const commands = await page.evaluate(() => window.lightRequests);
  expect(commands.filter(({ command }) => command.includes("discover")).map(({ command }) => command))
    .toEqual(["yeelight_discover", "hue_discover"]);
  expect(commands).toContainEqual({ command: "lan_light_update", args: { id: "hue-fixture", update: { on: false } } });
});

test("discovers and controls a WiZ bulb alongside a paired Tapo", async ({ page }) => {
  test.setTimeout(35_000);
  await page.goto("/?state=expanded");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  const signingKey = "0123456789abcdef0123456789abcdef";
  await expect(tools.getByText("Desk lamp", { exact: true })).toBeVisible();
  await tools.getByRole("button", { name: "Add light", exact: true }).click();
  await expectBrandPicker(tools);
  await tools.getByRole("button", { name: "WiZ", exact: true }).click();
  await tools.getByRole("button", { name: "Find bulbs", exact: true }).click();
  await tools.getByRole("button", { name: "Advanced", exact: true }).click();
  const key = tools.getByLabel("Home security key", { exact: true });
  await expect(key).toHaveAttribute("type", "password");
  await key.fill(signingKey);
  await tools.getByRole("button", { name: "Connect light", exact: true }).click();
  await expect(savedLight(tools, "Studio lamp")).toHaveAttribute("aria-checked", "true");

  await tools.getByRole("slider", { name: "Brightness", exact: true }).press("End");
  await expect(tools.getByRole("slider", { name: "Brightness", exact: true })).toHaveValue("100");
  await tools.getByRole("button", { name: "Blue", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Blue", exact: true })).toHaveAttribute("aria-pressed", "true");
  await tools.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Turn on", exact: true })).toBeVisible();

  await savedLight(tools, "Desk lamp").click();
  await expect(savedLight(tools, "Desk lamp")).toHaveAttribute("aria-checked", "true");
  await expect(tools.getByRole("button", { name: "Turn off", exact: true })).toBeVisible();
  await tools.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(tools.getByRole("button", { name: "Turn on", exact: true })).toBeVisible();
  await tools.getByRole("button", { name: "System", exact: true }).click();
  await tools.getByRole("button", { name: "Light", exact: true }).click();
  await savedLight(tools, "Studio lamp").click();
  await expect(savedLight(tools, "Studio lamp")).toHaveAttribute("aria-checked", "true");
  await expect(tools.getByRole("button", { name: "Turn on", exact: true })).toBeVisible();

  const requests = await page.evaluate(() => window.lightRequests);
  expect(requests.find(({ command }) => command === "wiz_connect")?.args)
    .toMatchObject({ ip: "192.168.3.29", signingKey });
  expect(requests).toContainEqual({ command: "wiz_state", args: { id: "aabbccddeeff" } });
  expect(requests.filter(({ command }) => command.endsWith("_update"))).toEqual([
    { command: "wiz_update", args: { id: "aabbccddeeff", update: { brightness: 100 } } },
    { command: "wiz_update", args: { id: "aabbccddeeff", update: { hue: 218, saturation: 90 } } },
    { command: "wiz_update", args: { id: "aabbccddeeff", update: { on: false } } },
    { command: "tapo_update", args: { id: "tapo", update: { on: false } } },
  ]);
  expect(requests.some(({ command }) => command === "tapo_connect" || command === "tapo_disconnect")).toBe(false);
});

test("connects a dimmable WiZ bulb by IP without credentials", async ({ page }) => {
  test.setTimeout(35_000);
  await page.goto("/?state=expanded");
  const tools = page.getByRole("region", { name: "Tools", exact: true });
  await tools.getByRole("button", { name: "Add light", exact: true }).click();
  await tools.getByRole("button", { name: "WiZ", exact: true }).click();
  await tools.getByRole("button", { name: "Enter IP manually", exact: true }).click();
  await expect(tools.getByLabel("Tapo email", { exact: true })).toHaveCount(0);
  await expect(tools.getByLabel("Password", { exact: true })).toHaveCount(0);
  await tools.getByLabel("Bulb IP", { exact: true }).fill("192.168.3.30");
  await tools.getByLabel("Name", { exact: true }).fill("Hall light");
  await tools.getByRole("button", { name: "Connect light", exact: true }).click();

  await expect(savedLight(tools, "Hall light")).toHaveAttribute("aria-checked", "true");
  await expect(tools.getByRole("slider", { name: "Brightness", exact: true })).toHaveAttribute("min", "10");
  await expect(tools.getByRole("group", { name: "Color", exact: true })).toHaveCount(0);
  await expect(tools.getByRole("group", { name: "White", exact: true })).toHaveCount(0);
  const requests = await page.evaluate(() => window.lightRequests);
  expect(requests.find(({ command }) => command === "wiz_connect")?.args)
    .toMatchObject({ ip: "192.168.3.30", name: "Hall light" });
  expect(requests.some(({ command }) => command.endsWith("_discover"))).toBe(false);
});
