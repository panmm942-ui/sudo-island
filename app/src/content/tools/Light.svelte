<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onDestroy, onMount } from "svelte";

  type LightState = {
    name: string;
    on: boolean;
    brightness: number;
    hue: number;
    saturation: number;
    colorTemperature: number;
    supportsBrightness?: boolean;
    supportsColor?: boolean;
    supportsTemperature?: boolean;
    minBrightness?: number;
    minColorTemperature?: number;
    maxColorTemperature?: number;
  };
  type Update = Partial<Pick<LightState, "on" | "brightness" | "hue" | "saturation" | "colorTemperature">>;
  type Provider = "tapo" | "wiz" | "yeelight" | "hue";
  type NetworkLight = { id: string; name: string; ip: string; model: string };
  type SavedLight = NetworkLight & { provider: Provider };
  type LanLight = NetworkLight & { provider: "yeelight" | "hue" };
  type DiscoveredBulb = { ip: string; model: string; paired?: boolean };
  type LinkedBridge = { bridgeId: string; lights: { id: number; name: string; model: string }[] };
  type SunsetLocation = { latitude: number; longitude: number; label: string };
  type SunsetTarget = { provider: Provider; id: string };
  type SunsetStatus = {
    enabled: boolean;
    latitude: number | null;
    longitude: number | null;
    locationName: string | null;
    nextSunset: string | null;
    lastError: string | null;
    timeZone?: string | null;
  };
  type SunsetAction = "checking" | "detecting" | "searching" | "saving" | "disabling";
  type Operation = "reading" | "discovering" | "pairing" | "updating" | "disconnecting" | "scheduling";

  const brands = { tapo: "Tapo", wiz: "WiZ", yeelight: "Xiaomi / Yeelight", hue: "Philips Hue" } as const;
  const providers = ["tapo", "wiz", "yeelight", "hue"] as const;
  const setupHelp = {
    tapo: "Set up your Tapo L530, L535 or L630 in the Tapo app first. You can add as many as you like.",
    wiz: "Set up the bulb in WiZ and allow local communication under Settings → Security.",
    yeelight: "Enable LAN Control in the Yeelight app. Compatible Xiaomi and Yeelight lights can be found without knowing their model.",
    hue: "Set up your lights on a Hue Bridge. For Philips bulbs marked WiZ Connected, choose WiZ.",
  };
  const discoveryHelp = {
    tapo: "No bulb found. Enter its IP from the Tapo app.",
    wiz: "No bulb found. Check Wi-Fi and local communication in WiZ, or enter the bulb’s IP.",
    yeelight: "No light found. Check Wi-Fi and LAN Control, or enter the light’s IP.",
    hue: "No bridge found. Enter the bridge IP from the Hue app.",
  };
  const colors = [
    { name: "Red", hue: 0, saturation: 95 },
    { name: "Amber", hue: 32, saturation: 95 },
    { name: "Green", hue: 132, saturation: 80 },
    { name: "Blue", hue: 218, saturation: 90 },
    { name: "Violet", hue: 281, saturation: 85 },
    { name: "Pink", hue: 328, saturation: 80 },
  ];
  const whitePresets = [
    { name: "Warm", kelvin: 2700 },
    { name: "Neutral", kelvin: 4000 },
    { name: "Cool", kelvin: 6500 },
  ];
  const sunsetClock: Intl.DateTimeFormatOptions = { dateStyle: "medium", timeStyle: "short" };

  let { onfocuschange }: { onfocuschange?: (focused: boolean) => void } = $props();
  let devices = $state<SavedLight[] | null>(null);
  let activeId = $state("");
  let lamp = $state<LightState | null>(null);
  /** The last state read from each light, shown at once when switching back to it. */
  let known = $state<Record<string, LightState>>({});
  let addingLight = $state(false);
  let provider = $state<Provider | null>(null);
  let operation = $state<Operation | null>(null);
  let error = $state("");
  let listError = $state("");
  let discoveryMessage = $state("");
  let discovered = $state<DiscoveredBulb[]>([]);
  let manual = $state(false);
  let ip = $state("");
  let name = $state("");
  let email = $state("");
  let password = $state("");
  /** The TP-Link account saved with a paired Tapo lamp, reused for the next one. */
  let tapoAccount = $state<string | null>(null);
  let otherAccount = $state(false);
  let signingKey = $state("");
  let advanced = $state(false);
  let bridge = $state<LinkedBridge | null>(null);
  let hueLightId = $state<number | "">("");
  let brightness = $state(100);
  let hue = $state(0);
  let saturation = $state(100);
  let customColor = $state(false);
  let manage = $state(false);
  let queuedUpdate: Update | null = null;
  let sunsetOpen = $state(false);
  let sunsetStatus = $state<SunsetStatus | null>(null);
  let sunsetEditing = $state(false);
  let sunsetBusy = $state<SunsetAction | null>(null);
  let sunsetError = $state("");
  let sunsetQuery = $state("");
  let sunsetResults = $state<SunsetLocation[] | null>(null);
  let sunsetLocation = $state<SunsetLocation | null>(null);
  let sunsetChoice = $state<number | "">("");
  let sunsetManual = $state(false);
  let sunsetLatitude = $state<number>();
  let sunsetLongitude = $state<number>();

  const activeLight = $derived(devices?.find((device) => device.id === activeId));
  const tapoLinked = $derived(devices?.some((device) => device.provider === "tapo") ?? false);
  const savedAccount = $derived(provider === "tapo" && !otherAccount ? tapoAccount : null);
  const selected = $derived(discovered.find((bulb) => bulb.ip === ip));
  const controlsLocked = $derived(!lamp?.on || !!operation);
  const slidersLocked = $derived(!lamp?.on || (operation !== null && operation !== "updating"));
  const whites = $derived.by(() => {
    const min = lamp?.minColorTemperature ?? 2500;
    const max = lamp?.maxColorTemperature ?? 6500;
    const clamped = whitePresets.map(({ name, kelvin }) => ({ name, kelvin: Math.max(min, Math.min(kelvin, max)) }));
    return clamped.filter((preset, index) => clamped.findIndex((other) => other.kelvin === preset.kelvin) === index);
  });
  const activeColor = $derived(lamp && lamp.colorTemperature === 0
    ? colors.find((preset) => lamp!.hue % 360 === preset.hue && lamp!.saturation === preset.saturation) : undefined);
  const activeWhite = $derived(lamp ? whites.find((preset) => preset.kelvin === lamp!.colorTemperature) : undefined);
  /** What the lamp is doing, in a line: "60% · Warm white". */
  const lightDetail = $derived.by(() => {
    if (!lamp) return "";
    if (!lamp.on) return activeLight ? [activeLight.model, brands[activeLight.provider]].filter(Boolean).join(" · ") : "";
    const parts: string[] = [];
    if (lamp.supportsBrightness !== false) parts.push(brightness + "%");
    if (lamp.colorTemperature > 0 && lamp.supportsTemperature !== false) {
      parts.push(activeWhite ? activeWhite.name + " white" : lamp.colorTemperature + " K");
    } else if (lamp.supportsColor !== false) {
      parts.push(activeColor?.name ?? "Custom color");
    }
    return parts.join(" · ");
  });
  const lightStatus = $derived(operation === "reading" ? "Refreshing…" : operation === "updating" ? "Updating…" : lamp?.on ? "On" : "Off");
  const lampTint = $derived(lamp ? tint(lamp) : "transparent");
  const sunsetInput = $derived.by(() => {
    if (!sunsetManual) return sunsetLocation;
    if (sunsetLatitude === undefined || !Number.isFinite(sunsetLatitude) || Math.abs(sunsetLatitude) > 90 ||
      sunsetLongitude === undefined || !Number.isFinite(sunsetLongitude) || Math.abs(sunsetLongitude) > 180) return null;
    return { latitude: sunsetLatitude, longitude: sunsetLongitude, label: "" };
  });
  const sunsetTime = $derived.by(() => {
    if (!sunsetStatus?.nextSunset) return "";
    const next = new Date(sunsetStatus.nextSunset);
    if (!Number.isFinite(next.getTime())) return "";
    try {
      return next.toLocaleString(undefined, { ...sunsetClock, timeZone: sunsetStatus.timeZone ?? undefined });
    } catch {
      return next.toLocaleString(undefined, sunsetClock);
    }
  });

  const focus = () => onfocuschange?.(true);
  const blur = () => onfocuschange?.(false);

  /** The glow of a white temperature, from candle-warm to daylight. */
  function kelvinTint(kelvin: number): string {
    const cool = Math.round(Math.max(0, Math.min(1, (kelvin - 2500) / 4000)) * 100);
    return "color-mix(in oklab, #dbe8ff " + cool + "%, #ffb163)";
  }

  function tint(state: LightState): string {
    if (state.colorTemperature > 0 || state.supportsColor === false) return kelvinTint(state.colorTemperature || 3200);
    return "hsl(" + (state.hue % 360) + " " + Math.max(35, state.saturation) + "% 62%)";
  }

  function savedCommand(device: SavedLight, action: "state" | "update" | "disconnect"): string {
    return (device.provider === "tapo" || device.provider === "wiz" ? device.provider : "lan_light") + "_" + action;
  }

  function sunsetTarget(device: SavedLight): SunsetTarget {
    return { provider: device.provider, id: device.id };
  }

  function setSunsetStatus(status: SunsetStatus | null): void {
    sunsetStatus = status;
    sunsetEditing = !!status && !status.enabled;
    sunsetLocation = status && status.latitude !== null && status.longitude !== null
      ? { latitude: status.latitude, longitude: status.longitude, label: status.locationName ?? "" }
      : null;
    sunsetManual = false;
    sunsetLatitude = undefined;
    sunsetLongitude = undefined;
    sunsetQuery = "";
    sunsetResults = null;
    sunsetChoice = "";
  }

  function closeSunset(): void {
    sunsetOpen = false;
    setSunsetStatus(null);
    sunsetError = "";
  }

  async function runSunset(next: SunsetAction, action: (device: SavedLight) => Promise<void>): Promise<void> {
    const device = activeLight;
    if (operation || !device) return;
    operation = "scheduling";
    sunsetBusy = next;
    sunsetError = "";
    try {
      await action(device);
    } catch (cause) {
      sunsetError = String(cause);
    } finally {
      sunsetBusy = null;
      operation = null;
    }
  }

  function loadSunset(): Promise<void> {
    return runSunset("checking", async (device) => {
      setSunsetStatus(await invoke<SunsetStatus>("light_sunset_status", sunsetTarget(device)));
    });
  }

  function toggleSunset(): void {
    if (sunsetOpen) {
      closeSunset();
      return;
    }
    sunsetOpen = true;
    manage = false;
    void loadSunset();
  }

  function detectSunsetLocation(): Promise<void> {
    return runSunset("detecting", async () => {
      const location = await invoke<SunsetLocation>("light_sunset_detect");
      sunsetLocation = location;
      sunsetManual = false;
      sunsetResults = null;
      sunsetChoice = "";
    });
  }

  function searchSunsetLocations(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const query = sunsetQuery.trim();
    if (query.length < 2) return Promise.resolve();
    return runSunset("searching", async () => {
      sunsetResults = null;
      sunsetChoice = "";
      sunsetResults = await invoke<SunsetLocation[]>("light_sunset_search", { query });
    });
  }

  function chooseSunsetLocation(): void {
    if (sunsetChoice === "" || !sunsetResults?.[sunsetChoice]) return;
    sunsetLocation = sunsetResults[sunsetChoice];
    sunsetManual = false;
    sunsetError = "";
  }

  function useSunsetCoordinates(): void {
    sunsetLatitude = sunsetLocation?.latitude;
    sunsetLongitude = sunsetLocation?.longitude;
    sunsetManual = true;
    sunsetError = "";
  }

  function saveSunset(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const location = sunsetInput;
    if (!location) return Promise.resolve();
    return runSunset("saving", async (device) => {
      setSunsetStatus(await invoke<SunsetStatus>("light_sunset_save", {
        ...sunsetTarget(device),
        latitude: location.latitude,
        longitude: location.longitude,
        locationName: location.label || undefined,
      }));
    });
  }

  function disableSunset(): Promise<void> {
    return runSunset("disabling", async (device) => {
      setSunsetStatus(await invoke<SunsetStatus>("light_sunset_disable", sunsetTarget(device)));
    });
  }

  function showLamp(device: SavedLight, state: LightState): void {
    lamp = state;
    known[device.id] = state;
    if (state.name && device.name !== state.name) {
      devices = devices?.map((saved) => saved.id === device.id ? { ...saved, name: state.name } : saved) ?? [];
    }
    brightness = state.brightness;
    hue = state.hue % 360;
    saturation = state.saturation || 100;
  }

  async function run(next: Operation, action: () => Promise<void>): Promise<void> {
    if (operation) return;
    operation = next;
    error = "";
    try {
      await action();
    } catch (cause) {
      error = String(cause);
    } finally {
      operation = null;
    }
  }

  async function loadDevices(): Promise<void> {
    const [tapo, wiz, lan] = await Promise.allSettled([
      invoke<NetworkLight[]>("tapo_list"),
      invoke<NetworkLight[]>("wiz_list"),
      invoke<LanLight[]>("lan_light_list"),
    ]);
    if (tapo.status === "rejected" && wiz.status === "rejected" && lan.status === "rejected") throw tapo.reason;

    const keep = (...kept: Provider[]) => (devices ?? []).filter((device) => kept.includes(device.provider));
    const tapoDevices = tapo.status === "fulfilled"
      ? (tapo.value ?? []).map((device): SavedLight => ({ ...device, provider: "tapo" })) : keep("tapo");
    const wizDevices = wiz.status === "fulfilled"
      ? (wiz.value ?? []).map((device): SavedLight => ({ ...device, provider: "wiz" })) : keep("wiz");
    const lanDevices = lan.status === "fulfilled" ? (lan.value ?? []) : keep("hue", "yeelight");
    devices = [...tapoDevices, ...wizDevices, ...lanDevices];
    listError = tapo.status === "rejected" ? "Could not read saved Tapo lights."
      : wiz.status === "rejected" ? "Could not read saved WiZ lights."
      : lan.status === "rejected" ? "Could not read saved Xiaomi or Hue lights." : "";
    if (!activeLight) activeId = devices[0]?.id ?? "";
  }

  async function readLamp(device: SavedLight): Promise<void> {
    try {
      showLamp(device, await invoke<LightState>(savedCommand(device, "state"), { id: device.id }));
    } catch (cause) {
      lamp = null;
      delete known[device.id];
      throw cause;
    }
  }

  function refresh(): Promise<void> {
    return run("reading", async () => {
      if (devices === null || listError) await loadDevices();
      if (activeLight) await readLamp(activeLight);
    });
  }

  function selectLight(id: string): Promise<void> {
    if (id === activeId && lamp) return Promise.resolve();
    return run("reading", async () => {
      activeId = id;
      lamp = null;
      manage = false;
      customColor = false;
      closeSunset();
      const device = activeLight;
      if (!device) return;
      if (known[id]) showLamp(device, known[id]);
      await readLamp(device);
    });
  }

  function changeAddress(): void {
    ip = "";
    manual = false;
    discoveryMessage = "";
    error = "";
    bridge = null;
    hueLightId = "";
  }

  function switchProvider(value: Provider | null): void {
    provider = value;
    discovered = [];
    name = "";
    email = "";
    password = "";
    otherAccount = false;
    signingKey = "";
    advanced = false;
    changeAddress();
    // A second Tapo lamp signs in with the account saved for the first.
    if (value === "tapo" && tapoLinked) {
      void invoke<string | null>("tapo_account").then((account) => (tapoAccount = account), () => (tapoAccount = null));
    } else {
      tapoAccount = null;
    }
  }

  function openSetup(): void {
    switchProvider(null);
    addingLight = true;
    manage = false;
    closeSunset();
  }

  function closeSetup(): void {
    switchProvider(null);
    addingLight = false;
    blur();
  }

  function useManualAddress(): void {
    changeAddress();
    manual = true;
  }

  function discover(): Promise<void> {
    const brand = provider;
    if (!brand) return Promise.resolve();
    return run("discovering", async () => {
      changeAddress();
      discovered = [];
      discovered = await invoke<DiscoveredBulb[]>(brand + "_discover");
      if (!discovered.length) {
        manual = true;
        discoveryMessage = discoveryHelp[brand];
        return;
      }
      const fresh = discovered.filter((bulb) => !bulb.paired);
      ip = fresh.length === 1 ? fresh[0].ip : "";
      if (!fresh.length) discoveryMessage = "Every bulb found is already added. Switch the new bulb on, or enter its IP.";
    });
  }

  function linkBridge(): Promise<void> {
    return run("pairing", async () => {
      bridge = await invoke<LinkedBridge>("hue_link_bridge", { ip: ip.trim() });
      hueLightId = "";
    });
  }

  function pair(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const brand = provider;
    if (!brand) return Promise.resolve();
    if (brand === "hue" && !bridge) return ip.trim() ? linkBridge() : Promise.resolve();
    return run("pairing", async () => {
      let device: SavedLight;
      let state: LightState;
      if (brand === "tapo") {
        const request = invoke<{ device: NetworkLight; state: LightState }>("tapo_connect",
          savedAccount ? { ip: ip.trim() } : { ip: ip.trim(), email: email.trim(), password });
        password = "";
        const result = await request;
        state = result.state;
        device = { ...result.device, provider: "tapo" };
      } else if (brand === "wiz") {
        const request = invoke<{ device: NetworkLight; state: LightState }>("wiz_connect", {
          ip: ip.trim(),
          name: name.trim() || undefined,
          signingKey: signingKey.trim() || undefined,
        });
        signingKey = "";
        const result = await request;
        state = result.state;
        device = { ...result.device, provider: "wiz" };
      } else {
        if (brand === "hue" && (!bridge || hueLightId === "")) return;
        const result = await invoke<{ device: LanLight; state: LightState }>(brand + "_connect", brand === "hue"
          ? { bridgeId: bridge?.bridgeId, lightId: hueLightId }
          : { ip: ip.trim(), name: name.trim() || undefined });
        state = result.state;
        device = result.device;
      }
      devices = [...(devices ?? []).filter((saved) => saved.id !== device.id), device];
      activeId = device.id;
      showLamp(device, state);
      customColor = false;
      closeSetup();
    });
  }

  function update(patch: Update): Promise<void> {
    const device = activeLight;
    if (!device) return Promise.resolve();
    if (operation === "updating") {
      // Sliders remain interactive during a write. Preserve their latest committed values.
      queuedUpdate = { ...queuedUpdate, ...patch };
      return Promise.resolve();
    }
    return run("updating", async () => {
      let next: Update | null = patch;
      try {
        while (next) {
          lamp = await invoke<LightState>(savedCommand(device, "update"), { id: device.id, update: next });
          next = queuedUpdate;
          queuedUpdate = null;
        }
      } finally {
        queuedUpdate = null;
        if (lamp) showLamp(device, lamp);
      }
    });
  }

  function disconnect(): Promise<void> {
    const device = activeLight;
    if (!device) return Promise.resolve();
    return run("disconnecting", async () => {
      await invoke(savedCommand(device, "disconnect"), { id: device.id });
      delete known[device.id];
      devices = devices?.filter((saved) => saved.id !== device.id) ?? [];
      activeId = devices[0]?.id ?? "";
      lamp = null;
      manage = false;
      customColor = false;
      closeSunset();
      if (activeLight) await readLamp(activeLight);
    });
  }

  onMount(() => {
    void refresh();
    let disposed = false;
    let off: (() => void) | undefined;
    void listen<SunsetTarget>("light-sunset-changed", ({ payload }) => {
      if (disposed || addingLight || !activeLight || activeLight.id !== payload.id || activeLight.provider !== payload.provider) return;
      if (!sunsetOpen) {
        void refresh();
      } else if (!sunsetEditing) {
        void loadSunset();
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else off = unlisten;
    }).catch(() => {});
    return () => {
      disposed = true;
      off?.();
    };
  });
  onDestroy(blur);
</script>

{#snippet icon(name: "plus" | "power" | "sunset" | "refresh" | "more" | "dim" | "bright" | "close")}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    {#if name === "plus"}<path d="M12 5v14M5 12h14" />
    {:else if name === "power"}<path d="M12 3.5v8M6.6 6.8a7.6 7.6 0 1 0 10.8 0" />
    {:else if name === "sunset"}<path d="M3.5 17.5h17M7 17.5a5 5 0 0 1 10 0M12 5.5V8M5 10.5l1.7 1.7M19 10.5l-1.7 1.7M9 21h6" />
    {:else if name === "refresh"}<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.6h-4.6" />
    {:else if name === "more"}<path d="M6 12h.01M12 12h.01M18 12h.01" stroke-width="2.8" />
    {:else if name === "dim"}<circle cx="12" cy="12" r="3" /><path d="M12 5.5v.5M12 18v.5M5.5 12H6M18 12h.5" />
    {:else if name === "bright"}<circle cx="12" cy="12" r="4" /><path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2M5.3 5.3l1.5 1.5M17.2 17.2l1.5 1.5M5.3 18.7l1.5-1.5M17.2 6.8l1.5-1.5" />
    {:else}<path d="m6.5 6.5 11 11M17.5 6.5l-11 11" />{/if}
  </svg>
{/snippet}


<section class="light" aria-label="Smart lights" onwheel={(event) => event.stopPropagation()}>
  {#if devices === null}
    {#if operation === "reading"}
      <p class="empty" role="status">Checking for saved lights…</p>
    {:else}
      <div class="empty"><button type="button" class="small-btn" onclick={refresh}>Retry</button></div>
    {/if}
  {:else if addingLight || !devices.length}
    <form class="setup" autocomplete="off" onsubmit={pair}>
      {#if !provider}<p class="title heading">Choose your light brand</p>{/if}
      <div class="bar">
        <div class="brands grow" role="group" aria-label="Light brand">
          {#each providers as brand (brand)}
            <button type="button" class="brand" aria-pressed={provider === brand} disabled={!!operation}
              onclick={() => switchProvider(brand)}>{brands[brand]}</button>
          {/each}
        </div>
        {#if addingLight}
          <button type="button" class="icon-btn" aria-label="Cancel" title="Cancel" disabled={!!operation} onclick={closeSetup}>
            {@render icon("close")}
          </button>
        {/if}
      </div>

      {#if !provider}
        <p class="status">Pick the app your bulb was set up with. For Philips bulbs marked WiZ Connected, choose WiZ.</p>
      {:else if provider === "hue" && bridge}
        {#if bridge.lights.length}
          <div class="field-row">
            <label for="hue-light">Hue light</label>
            <select id="hue-light" class="field" bind:value={hueLightId} disabled={!!operation}
              onpointerdown={focus} onfocus={focus} onblur={blur}>
              <option value="">Choose a light</option>
              {#each bridge.lights as choice (choice.id)}
                <option value={choice.id}>{choice.name}</option>
              {/each}
            </select>
          </div>
          <div class="actions">
            <button type="submit" class="small-btn accent" disabled={!!operation || hueLightId === ""}>
              {operation === "pairing" ? "Connecting…" : "Connect light"}
            </button>
            <button type="button" class="small-btn" disabled={!!operation} onclick={changeAddress}>Change bridge</button>
          </div>
        {:else}
          <p class="status">No lights are set up on this bridge. Add one in the Hue app first.</p>
          <div class="actions">
            <button type="button" class="small-btn" disabled={!!operation} onclick={changeAddress}>Change bridge</button>
          </div>
        {/if}
      {:else}
        {#if !discovered.length && !manual && !discoveryMessage}
          <p class="status">{setupHelp[provider]}</p>
        {/if}

        {#if manual}
          <div class="field-row">
            <label for="light-ip">{provider === "hue" ? "Bridge IP" : "Bulb IP"}</label>
            <input id="light-ip" class="field" type="text" inputmode="decimal" spellcheck="false" required maxlength="15"
              placeholder="192.168.1.20" bind:value={ip}
              disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
          </div>
        {:else if discovered.length}
          <div class="found" role="radiogroup" aria-label={provider === "hue" ? "Found bridges" : "Found bulbs"}>
            {#each discovered as bulb (bulb.ip)}
              <button type="button" class="found-row" role="radio" aria-checked={ip === bulb.ip} disabled={!!operation}
                onclick={() => (ip = bulb.ip)}>
                <span class="radio" aria-hidden="true"></span>
                <span class="title grow">{bulb.model}</span>
                {#if bulb.paired}<span class="tag">Added</span>{/if}
                <span class="caption tabular">{bulb.ip}</span>
              </button>
            {/each}
          </div>
        {/if}

        {#if discoveryMessage && !error}<p class="status" role="status">{discoveryMessage}</p>{/if}

        {#if manual || ip}
          {#if provider === "tapo"}
            {#if savedAccount}
              <div class="account">
                <p class="status grow account-name">Signs in as <span class="strong">{savedAccount}</span></p>
                <button type="button" class="small-btn ghost" disabled={!!operation}
                  onclick={() => (otherAccount = true)}>Use another account</button>
              </div>
            {:else}
              <div class="field-row">
                <label for="tapo-email">Tapo email</label>
                <input id="tapo-email" class="field" type="email" spellcheck="false" required bind:value={email}
                  disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
              </div>
              <div class="field-row">
                <label for="tapo-password">Password</label>
                <input id="tapo-password" class="field" type="password" autocomplete="off" required bind:value={password}
                  disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
              </div>
            {/if}
          {:else if provider === "hue"}
            <p class="status">Press the link button on the Hue Bridge, then choose Connect bridge.</p>
          {:else}
            <div class="field-row">
              <label for="light-name">Name</label>
              <input id="light-name" class="field" type="text" placeholder="Optional" maxlength="48" bind:value={name}
                disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
            </div>
            {#if provider === "wiz" && advanced}
              <div class="field-row" id="wiz-signing">
                <label for="wiz-key">Home security key</label>
                <input id="wiz-key" class="field" type="password" maxlength="128" bind:value={signingKey} aria-describedby="wiz-key-help"
                  disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
              </div>
              <p class="status" id="wiz-key-help">For verified controls, export your home from Local integrations in the WiZ app and use the <code>udp_signing_key</code>.</p>
            {/if}
          {/if}
        {/if}

        <div class="actions">
          {#if manual || ip}
            <button type="submit" class="small-btn accent" disabled={!!operation || !ip.trim()}>
              {provider === "tapo"
                ? (operation === "pairing" ? "Pairing…" : "Pair light")
                : operation === "pairing" ? "Connecting…" : provider === "hue" ? "Connect bridge" : "Connect light"}
            </button>
          {/if}
          <button type="button" class="small-btn" class:accent={!manual && !ip} disabled={!!operation} onclick={discover}>
            {operation === "discovering" ? "Finding…" : discovered.length ? "Find again" : provider === "hue" ? "Find bridges" : "Find bulbs"}
          </button>
          {#if !manual}
            <button type="button" class="small-btn ghost" disabled={!!operation} onclick={useManualAddress}>Enter IP manually</button>
          {/if}
          {#if provider === "wiz" && (manual || ip)}
            <button type="button" class="small-btn ghost" aria-expanded={advanced} aria-controls={advanced ? "wiz-signing" : undefined}
              disabled={!!operation} onclick={() => {
                advanced = !advanced;
                if (!advanced) signingKey = "";
              }}>Advanced</button>
          {/if}
        </div>
      {/if}
      {#if listError}
        <div class="actions"><button type="button" class="small-btn" disabled={!!operation} onclick={refresh}>Retry saved lights</button></div>
      {/if}
    </form>
  {:else if activeLight}
    <div class="bar">
      <div class="lights grow" role="radiogroup" aria-label="Saved lights" onwheel={(event) => {
        if (event.currentTarget.scrollWidth > event.currentTarget.clientWidth) event.currentTarget.scrollLeft += event.deltaY || event.deltaX;
      }}>
        {#each devices as device (device.id)}
          <button type="button" class="chip" role="radio" aria-checked={device.id === activeId} disabled={!!operation && device.id !== activeId}
            title={[device.name, device.model, brands[device.provider]].filter(Boolean).join(" · ")}
            onclick={() => selectLight(device.id)}>
            <span class="dot" class:on={known[device.id]?.on} style:--tint={known[device.id] ? tint(known[device.id]) : "transparent"} aria-hidden="true"></span>
            <span class="chip-name">{device.name}</span>
          </button>
        {/each}
      </div>
      <button type="button" class="small-btn add" disabled={!!operation} onclick={openSetup}>
        {@render icon("plus")}<span>Add light</span>
      </button>
    </div>

    <div class="hero" class:on={lamp?.on} style:--tint={lampTint}>
      <button type="button" class="power" aria-label={lamp?.on ? "Turn off" : "Turn on"} aria-pressed={lamp?.on ?? false}
        disabled={!lamp || !!operation} onclick={() => lamp && update({ on: !lamp.on })}>
        {@render icon("power")}
      </button>
      <div class="grow summary">
        {#if lamp}
          <p class="state" role="status">{lightStatus}</p>
          <p class="caption">{lightDetail}</p>
        {:else}
          <p class="state" role="status">{operation === "reading" ? "Connecting…" : "Unavailable"}</p>
          <p class="caption">{operation === "reading" ? "Reading saved light" : "Check its power and Wi-Fi, then refresh."}</p>
        {/if}
      </div>
      <button type="button" class="icon-btn" aria-label="Sunset schedule" title="Turn on at sunset"
        aria-controls={sunsetOpen ? "light-sunset" : undefined} aria-expanded={sunsetOpen}
        disabled={!!operation} onclick={toggleSunset}>{@render icon("sunset")}</button>
      <button type="button" class="icon-btn" class:spin={operation === "reading"} aria-label={lamp ? "Refresh" : "Retry"} title="Refresh"
        disabled={!!operation} onclick={refresh}>{@render icon("refresh")}</button>
      <button type="button" class="icon-btn" aria-label="Connection" title="Connection"
        aria-controls={manage ? "light-connection" : undefined} aria-expanded={manage}
        onclick={() => { manage = !manage; if (manage) closeSunset(); }}>{@render icon("more")}</button>
    </div>

    {#if manage}
      <div class="panel" id="light-connection">
        <div class="grow summary">
          <p class="title">{activeLight.name}</p>
          <p class="caption tabular">{[brands[activeLight.provider], activeLight.model, activeLight.ip === "0.0.0.0" ? "" : activeLight.ip].filter(Boolean).join(" · ")}</p>
        </div>
        <button type="button" class="small-btn danger" disabled={!!operation} onclick={disconnect}>
          {operation === "disconnecting" ? "Disconnecting…" : "Disconnect"}
        </button>
      </div>
    {:else if !sunsetOpen}
      {#if lamp}
        {#if lamp.supportsBrightness !== false}
          <div class="slider" style:--fill={((brightness - (lamp.minBrightness ?? 1)) / (100 - (lamp.minBrightness ?? 1))) * 100 + "%"}>
            <span class="end">{@render icon("dim")}</span>
            <input id="light-brightness" class="level" type="range" aria-label="Brightness" min={lamp.minBrightness ?? 1} max="100"
              bind:value={brightness} disabled={slidersLocked}
              aria-valuetext={brightness + "%"} onpointerdown={focus} onfocus={focus} onblur={blur}
              onchange={() => update({ brightness })} />
            <span class="end">{@render icon("bright")}</span>
            <span class="percent tabular" aria-hidden="true">{brightness}%</span>
          </div>
        {/if}
        {#if lamp.supportsColor !== false || lamp.supportsTemperature !== false}
          {@const mixing = customColor && lamp.on && lamp.supportsColor !== false}
          <div class="palette">
            {#if !mixing && lamp.supportsTemperature !== false}
              <div class="choices" role="group" aria-label="White">
                {#each whites as preset (preset.kelvin)}
                  <button type="button" class="swatch" aria-label={preset.name} title={preset.name + " white · " + preset.kelvin + " K"}
                    aria-pressed={lamp.colorTemperature === preset.kelvin}
                    style:--swatch={kelvinTint(preset.kelvin)} disabled={controlsLocked}
                    onclick={() => update({ colorTemperature: preset.kelvin })}><span aria-hidden="true"></span></button>
                {/each}
              </div>
              {#if lamp.supportsColor !== false}<span class="rule" aria-hidden="true"></span>{/if}
            {/if}
            {#if lamp.supportsColor !== false}
              <div class="choices" class:grow={mixing} role="group" aria-label="Color">
                {#if !mixing}
                  {#each colors as preset (preset.name)}
                    <button type="button" class="swatch" aria-label={preset.name} title={preset.name}
                      aria-pressed={activeColor === preset}
                      style:--swatch={"hsl(" + preset.hue + " " + preset.saturation + "% 60%)"} disabled={controlsLocked}
                      onclick={() => update({ hue: preset.hue, saturation: preset.saturation })}><span aria-hidden="true"></span></button>
                  {/each}
                {/if}
                <button type="button" class="swatch wheel" aria-label="Custom" title={mixing ? "Back to presets" : "Custom color"}
                  aria-controls={mixing ? "light-custom-color" : undefined} aria-expanded={mixing}
                  disabled={!lamp.on} onclick={() => (customColor = !customColor)}><span aria-hidden="true"></span></button>
                {#if mixing}
                  <div class="custom-color grow" id="light-custom-color" style:--hue={hue}>
                    <div class="slider">
                      <label class="end text" for="light-hue">Hue</label>
                      <input id="light-hue" class="hue" type="range" min="0" max="359" bind:value={hue} disabled={slidersLocked}
                        aria-valuetext={hue + " degrees"} onpointerdown={focus} onfocus={focus} onblur={blur}
                        onchange={() => update({ hue, saturation })} />
                    </div>
                    <div class="slider">
                      <label class="end text" for="light-saturation" title="Saturation"><span aria-hidden="true">Sat</span><span class="sr">Saturation</span></label>
                      <input id="light-saturation" class="saturation" type="range" min="1" max="100" bind:value={saturation} disabled={slidersLocked}
                        aria-valuetext={saturation + "%"} onpointerdown={focus} onfocus={focus} onblur={blur}
                        onchange={() => update({ hue, saturation })} />
                    </div>
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}
      {/if}
    {:else}
      <section id="light-sunset" class="sunset" aria-label="Sunset schedule">
        {#if !sunsetStatus}
          {#if sunsetBusy === "checking"}
            <p class="status" role="status">Checking the schedule…</p>
          {:else}
            <div class="actions"><button type="button" class="small-btn" onclick={loadSunset}>Retry schedule</button></div>
          {/if}
        {:else}
          {#if sunsetStatus.enabled}
            <p class="title">On at sunset{sunsetStatus.locationName ? " · " + sunsetStatus.locationName : ""}</p>
            <p class="status" role="status">{sunsetTime ? "Next sunset · " + sunsetTime : "Next sunset is unavailable."}</p>
            {#if sunsetStatus.lastError}<p class="status error">{sunsetStatus.lastError}</p>{/if}
          {:else}
            <p class="title">Turn on this light at sunset</p>
          {/if}

          {#if sunsetEditing}
            <div class="actions">
              <button type="button" class="small-btn" disabled={!!operation} onclick={detectSunsetLocation}>
                {sunsetBusy === "detecting" ? "Finding location…" : "Use Windows location"}
              </button>
              {#if !sunsetManual}
                <button type="button" class="small-btn ghost" disabled={!!operation} onclick={useSunsetCoordinates}>Enter coordinates</button>
              {/if}
            </div>
            <form class="sunset-search" autocomplete="off" onsubmit={searchSunsetLocations}>
              <input class="field" maxlength="100" aria-label="City or town" placeholder="City or town" bind:value={sunsetQuery}
                disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
              <button type="submit" class="small-btn" disabled={!!operation || sunsetQuery.trim().length < 2}>
                {sunsetBusy === "searching" ? "Searching…" : "Search"}
              </button>
            </form>
            {#if sunsetResults?.length}
              <div class="field-row">
                <label for="sunset-results">Matches</label>
                <select id="sunset-results" class="field" bind:value={sunsetChoice} onchange={chooseSunsetLocation}
                  disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur}>
                  <option value="">Choose a location</option>
                  {#each sunsetResults as result, index (index)}
                    <option value={index}>{result.label}</option>
                  {/each}
                </select>
              </div>
            {:else if sunsetResults}
              <p class="status" role="status">No matching towns. Try a nearby town or coordinates.</p>
            {/if}
            {#if sunsetResults}
              <p class="status">City search by <a href="https://open-meteo.com/en/docs/geocoding-api" target="_blank" rel="noreferrer">Open-Meteo</a>
                using <a href="https://www.geonames.org/" target="_blank" rel="noreferrer">GeoNames</a>.</p>
            {/if}
            <form autocomplete="off" onsubmit={saveSunset}>
              {#if sunsetManual}
                <div class="field-row">
                  <label for="sunset-latitude">Latitude</label>
                  <input id="sunset-latitude" class="field" type="number" min="-90" max="90" step="any" required
                    bind:value={sunsetLatitude} disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
                </div>
                <div class="field-row">
                  <label for="sunset-longitude">Longitude</label>
                  <input id="sunset-longitude" class="field" type="number" min="-180" max="180" step="any" required
                    bind:value={sunsetLongitude} disabled={!!operation} onpointerdown={focus} onfocus={focus} onblur={blur} />
                </div>
              {:else if sunsetLocation}
                <p class="status" role="status">Selected · {sunsetLocation.label || "Saved coordinates"}</p>
              {/if}
              <div class="actions">
                <button type="submit" class="small-btn accent" disabled={!!operation || !sunsetInput}>
                  {sunsetBusy === "saving" ? "Saving…" : sunsetStatus.enabled ? "Save schedule" : "Enable schedule"}
                </button>
                {#if sunsetStatus.enabled}
                  <button type="button" class="small-btn ghost" disabled={!!operation}
                    onclick={() => sunsetStatus && setSunsetStatus(sunsetStatus)}>Cancel</button>
                {/if}
              </div>
            </form>
          {:else}
            <div class="actions">
              <button type="button" class="small-btn" disabled={!!operation} onclick={() => (sunsetEditing = true)}>Change location</button>
              <button type="button" class="small-btn ghost" disabled={!!operation} onclick={disableSunset}>
                {sunsetBusy === "disabling" ? "Disabling…" : "Disable schedule"}
              </button>
            </div>
          {/if}
        {/if}
        {#if sunsetError}<p class="status error" role="alert">{sunsetError}</p>{/if}
        <p class="status fine">Runs while Sudo Island is open and your PC is awake. Approximate coordinates are sent to
          <a href="https://sunrise-sunset.org/" target="_blank" rel="noreferrer">Sunrise-Sunset.org</a> to calculate sunset.</p>
      </section>
    {/if}
  {/if}
  {#if error}<p class="status error" role="alert">{error}</p>{/if}
  {#if listError}<p class="status" role="status">{listError}</p>{/if}
</section>

<style>
  /* Longer forms scroll with the wheel. No scroll bar is drawn: it crowded the controls. */
  .light { height: 100%; overflow: hidden auto; scrollbar-width: none; display: grid; align-content: start; gap: 6px; padding: 1px 3px 2px; }
  form { display: grid; gap: 5px; }
  .custom-color { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px; }
  .custom-color .slider { gap: 6px; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .bar, .actions, .choices, .hero, .panel, .account, .palette, .slider { display: flex; align-items: center; min-width: 0; }
  .bar, .actions, .account { gap: 6px; }
  .actions { flex-wrap: wrap; }
  .heading { font: var(--type-title); letter-spacing: var(--tracking-tight); }
  .status, .percent, .field-row > label, .end.text { font: var(--type-caption); color: var(--text-secondary); margin: 0; }
  .status.fine { opacity: .8; }
  .strong { color: var(--text-primary); }
  .account-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .error { color: #ffb4b4; }
  .empty { margin: 0; }
  :disabled { opacity: .5; cursor: default; }
  .field-row > :focus-visible, button:focus-visible { outline-offset: 0; }
  .ghost { background: transparent; box-shadow: inset 0 0 0 1px #ffffff26; }
  .danger { background: #ff6b6b26; color: #ffc9c9; }
  .danger:hover:not(:disabled) { background: #ff6b6b40; }
  .small-btn[aria-expanded="true"] { box-shadow: inset 0 0 0 1px var(--text-secondary); }
  svg { width: 15px; height: 15px; }

  /* Saved lights */
  .lights { display: flex; gap: 4px; overflow-x: auto; scrollbar-width: none; padding: 1px; margin: -1px; mask-image: linear-gradient(to right, #000 calc(100% - 14px), transparent); }
  .chip { flex: none; display: flex; align-items: center; gap: 6px; max-width: 150px; height: 26px; padding: 0 11px 0 9px; border: 0; border-radius: var(--radius-pill); background: transparent; color: var(--text-secondary); font: 600 11.5px/1 var(--font-sans); cursor: pointer; transition: background 140ms var(--ease-out), color 140ms var(--ease-out); }
  .chip:hover:not(:disabled) { color: var(--text-primary); background: var(--surface-control); }
  .chip[aria-checked="true"] { color: var(--text-primary); background: var(--surface-control-hover); opacity: 1; }
  .chip-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dot { flex: none; width: 8px; height: 8px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--text-secondary); }
  .dot.on { background: var(--tint); box-shadow: 0 0 6px var(--tint); }
  .add { display: flex; align-items: center; gap: 4px; padding-left: 7px; }
  .add svg { width: 13px; height: 13px; }

  /* The light itself */
  .hero { gap: 4px; padding: 4px 6px 4px 6px; border-radius: 18px; background: #ffffff0c; box-shadow: inset 0 0 0 1px #ffffff12; transition: background 240ms var(--ease-out); }
  .hero.on { background: radial-gradient(120% 160% at 0% 50%, color-mix(in oklab, var(--tint) 24%, transparent), transparent 62%), #ffffff0c; }
  .power { flex: none; display: grid; place-items: center; width: 36px; height: 36px; margin-right: 6px; padding: 0; border: 0; border-radius: 50%; background: var(--surface-control); color: var(--text-primary); cursor: pointer; transition: background 200ms var(--ease-out), box-shadow 200ms var(--ease-out), color 200ms var(--ease-out); }
  .power svg { width: 17px; height: 17px; }
  .power:hover:not(:disabled) { background: var(--surface-control-hover); }
  .power[aria-pressed="true"] { background: var(--tint); color: oklch(0.22 0.02 70); text-shadow: none; box-shadow: 0 0 16px color-mix(in oklab, var(--tint) 55%, transparent); }
  .power[aria-pressed="true"]:hover:not(:disabled) { background: color-mix(in oklab, var(--tint) 88%, #fff); }
  .summary { display: grid; gap: 1px; }
  .state { margin: 0; font: var(--type-title); letter-spacing: var(--tracking-tight); }
  .hero .icon-btn[aria-expanded="true"] { background: var(--surface-control-hover); opacity: 1; }
  .spin svg { animation: spin 900ms linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .panel { gap: 8px; padding: 8px 8px 8px 12px; border-radius: 14px; background: #ffffff0c; box-shadow: inset 0 0 0 1px #ffffff12; }

  /* Sliders */
  .slider { gap: 8px; min-height: 22px; padding-inline: 4px; }
  .slider > input { flex: 1; min-width: 0; height: 22px; }
  .end { flex: none; display: grid; place-items: center; color: var(--text-secondary); }
  .end.text { justify-items: start; }
  .percent { flex: none; min-width: 34px; text-align: right; }
  /* These outrank the Tools tab's shared slider look. */
  .light .slider > input[type="range"]::-webkit-slider-runnable-track { height: 6px; border-radius: 3px; }
  .light .slider > input[type="range"]::-webkit-slider-thumb { width: 14px; height: 14px; margin-top: -4px; }
  .light .slider > input[type="range"].level::-webkit-slider-runnable-track { background: linear-gradient(to right, #fff var(--fill, 0%), #ffffff30 var(--fill, 0%)); }
  .light .slider > input[type="range"].hue::-webkit-slider-runnable-track { background: linear-gradient(to right, #f55, #ff5, #5f5, #5ff, #55f, #f5f, #f55); }
  .light .slider > input[type="range"].saturation::-webkit-slider-runnable-track { background: linear-gradient(to right, #fff, hsl(var(--hue, 0) 100% 55%)); }

  /* Whites and colors */
  .palette { gap: 6px; }
  .choices { gap: 2px; min-height: 27px; }
  .rule { flex: none; width: 1px; height: 16px; background: #ffffff24; }
  .swatch { flex: none; display: grid; place-items: center; width: 27px; height: 27px; padding: 0; border: 0; border-radius: 50%; background: transparent; cursor: pointer; }
  .swatch > span { width: 18px; height: 18px; border-radius: 50%; background: var(--swatch); box-shadow: inset 0 0 0 1px #ffffff2e; transition: transform 140ms var(--ease-out); }
  .swatch:hover:not(:disabled) > span { transform: scale(1.12); }
  .swatch[aria-pressed="true"], .swatch[aria-expanded="true"] { box-shadow: inset 0 0 0 2px var(--text-primary); }
  .swatch[aria-pressed="true"] > span, .swatch[aria-expanded="true"] > span { transform: scale(.82); }
  .wheel > span { background: conic-gradient(#f55, #ff5, #5f5, #5ff, #55f, #f5f, #f55); }

  /* Setup */
  .brands { display: flex; gap: 2px; padding: 2px; border-radius: 14px; background: #ffffff0c; }
  .brand { flex: 1 1 auto; height: 24px; min-width: 0; padding: 0 6px; border: 0; border-radius: 11px; background: transparent; color: var(--text-secondary); font: 600 11.5px/1 var(--font-sans); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; cursor: pointer; transition: background 140ms var(--ease-out), color 140ms var(--ease-out); }
  .brand:hover:not(:disabled) { color: var(--text-primary); background: var(--surface-control); }
  .brand[aria-pressed="true"] { color: var(--text-primary); background: var(--surface-control-hover); box-shadow: inset 0 0 0 1px #ffffff1f; }
  .field-row { display: grid; grid-template-columns: 82px minmax(0, 1fr); align-items: center; gap: 6px; min-width: 0; }
  .light .field-row > .field, .light .sunset-search > .field { height: 26px; min-width: 0; }
  select.field { color-scheme: dark; }
  .found { display: grid; gap: 2px; }
  .found-row { display: flex; align-items: center; gap: 8px; min-width: 0; height: 26px; padding: 0 10px; border: 0; border-radius: 10px; background: transparent; color: inherit; text-align: left; cursor: pointer; }
  .found-row:hover:not(:disabled) { background: #ffffff0d; }
  .found-row[aria-checked="true"] { background: var(--surface-control); }
  .radio { flex: none; width: 12px; height: 12px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--text-secondary); }
  .found-row[aria-checked="true"] .radio { box-shadow: inset 0 0 0 3.5px var(--text-primary); }
  .tag { flex: none; padding: 2px 7px; border-radius: var(--radius-pill); background: var(--surface-control); font: 600 10px/1.2 var(--font-sans); color: var(--text-secondary); }

  /* Sunset schedule */
  .sunset { display: grid; gap: 6px; }
  .sunset-search { grid-template-columns: minmax(0, 1fr) auto; align-items: center; }
  .sunset a { color: var(--text-primary); }

  @media (prefers-reduced-motion: reduce) {
    .spin svg { animation: none; }
    .chip, .brand, .power, .hero, .swatch > span { transition: none; }
  }
</style>
