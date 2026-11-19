use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::light_types::{DiscoveryResult, LampState, LampUpdate, local_ipv4};
use crate::{extension_net::credentials, hue, yeelight};

const PREFIX: &str = "SudoIsland/LanLight/";
const MISSING: &str = "This light is no longer saved. Add it again in Tools → Light.";

#[derive(Default)]
pub struct Controller(Mutex<Option<PendingBridge>>);

struct PendingBridge {
    expires: Instant,
    bridge: hue::Bridge,
    choices: Vec<hue::LightChoice>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
enum Device {
    Yeelight(yeelight::Device),
    Hue(hue::Device),
}

#[derive(Deserialize, Serialize)]
struct Profile {
    device: Device,
    name: String,
    model: String,
}

#[derive(Serialize)]
pub struct SavedLight {
    id: String,
    provider: &'static str,
    name: String,
    ip: Ipv4Addr,
    model: String,
}

#[derive(Serialize)]
pub struct ConnectedLight {
    device: SavedLight,
    state: LampState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedBridge {
    bridge_id: String,
    lights: Vec<hue::LightChoice>,
}

fn check_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "island" {
        Ok(())
    } else {
        Err("Open the light controls from Tools → Light.".into())
    }
}

fn valid_id(id: &str) -> bool {
    let Some((kind, value)) = id.split_once('-') else {
        return false;
    };
    matches!((kind, value.len()), ("yeelight", 16) | ("hue", 64))
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn target(id: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("Invalid saved light identifier.".into());
    }
    Ok(format!("{PREFIX}{id}"))
}

impl Device {
    fn id(&self) -> String {
        match self {
            Self::Yeelight(device) => format!("yeelight-{}", device.id),
            Self::Hue(device) => {
                let hash = Sha256::digest(format!("{}|{}", device.bridge.id, device.unique_id));
                format!("hue-{hash:x}")
            }
        }
    }
}

impl Profile {
    fn saved(&self) -> SavedLight {
        let (provider, ip) = match &self.device {
            Device::Yeelight(device) => ("yeelight", device.ip),
            Device::Hue(device) => ("hue", device.bridge.ip),
        };
        SavedLight {
            id: self.device.id(),
            provider,
            name: self.name.clone(),
            ip,
            model: self.model.clone(),
        }
    }
}

fn read(id: &str) -> Result<Profile, String> {
    let blob = credentials::read(&target(id)?)?
        .map(Zeroizing::new)
        .ok_or(MISSING)?;
    let profile: Profile = serde_json::from_str(&blob)
        .map_err(|_| "The saved light is unreadable. Remove it and add it again.")?;
    if profile.device.id() != id {
        return Err(
            "The saved light identifier does not match. Remove it and add it again.".into(),
        );
    }
    Ok(profile)
}

fn profiles() -> Result<Vec<Profile>, String> {
    credentials::targets(PREFIX)?
        .into_iter()
        .filter_map(|target| {
            target
                .strip_prefix(PREFIX)
                .filter(|id| valid_id(id))
                .map(read)
        })
        .collect()
}

fn save(
    device: Device,
    model: String,
    mut state: LampState,
    name: Option<String>,
) -> Result<ConnectedLight, String> {
    state.name = name.unwrap_or(state.name);
    let profile = Profile {
        device,
        model,
        name: state.name.clone(),
    };
    let label = match &profile.device {
        Device::Yeelight(_) => "Sudo Island Xiaomi / Yeelight",
        Device::Hue(_) => "Sudo Island Philips Hue",
    };
    let blob =
        Zeroizing::new(serde_json::to_string(&profile).map_err(|_| "Could not save the light.")?);
    credentials::write_labeled(&target(&profile.device.id())?, label, blob.as_str())?;
    Ok(ConnectedLight {
        device: profile.saved(),
        state,
    })
}

#[tauri::command]
pub async fn lan_light_list(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Vec<SavedLight>, String> {
    check_window(&window)?;
    let _guard = controller.0.lock().await;
    let mut lights: Vec<_> = profiles()?.iter().map(Profile::saved).collect();
    lights.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(lights)
}

#[tauri::command]
pub async fn yeelight_discover(
    window: tauri::WebviewWindow,
) -> Result<Vec<DiscoveryResult>, String> {
    check_window(&window)?;
    yeelight::discover().await
}

#[tauri::command]
pub async fn yeelight_connect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    ip: String,
    name: Option<String>,
) -> Result<ConnectedLight, String> {
    check_window(&window)?;
    let ip = local_ipv4(&ip)?;
    let name = name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    if name
        .as_ref()
        .is_some_and(|name| name.chars().count() > 48 || name.chars().any(char::is_control))
    {
        return Err("Use a light name of up to 48 characters.".into());
    }
    let _guard = controller.0.lock().await;
    let (device, state) = yeelight::connect(ip).await?;
    let model = device.model.clone();
    save(Device::Yeelight(device), model, state, name)
}

#[tauri::command]
pub async fn hue_discover(window: tauri::WebviewWindow) -> Result<Vec<DiscoveryResult>, String> {
    check_window(&window)?;
    hue::discover().await
}

#[tauri::command]
pub async fn hue_link_bridge(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    ip: String,
) -> Result<LinkedBridge, String> {
    check_window(&window)?;
    let ip = local_ipv4(&ip)?;
    let mut pending = controller.0.lock().await;
    let previous = pending.take();
    let saved = profiles()?
        .into_iter()
        .find_map(|profile| match profile.device {
            Device::Hue(device) if device.bridge.ip == ip => Some(device.bridge),
            _ => None,
        });
    let existing = previous
        .as_ref()
        .filter(|link| link.expires > Instant::now() && link.bridge.ip == ip)
        .map(|link| &link.bridge)
        .or(saved.as_ref());
    let (bridge, lights) = hue::link(ip, existing).await?;
    let result = LinkedBridge {
        bridge_id: bridge.id.clone(),
        lights: lights.clone(),
    };
    *pending = Some(PendingBridge {
        bridge,
        choices: lights,
        expires: Instant::now() + Duration::from_secs(300),
    });
    Ok(result)
}

#[tauri::command]
pub async fn hue_connect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    bridge_id: String,
    light_id: u32,
) -> Result<ConnectedLight, String> {
    check_window(&window)?;
    let pending = controller.0.lock().await;
    let link = pending
        .as_ref()
        .filter(|link| link.bridge.id == bridge_id && link.expires > Instant::now())
        .ok_or("Connect the Hue Bridge first.")?;
    if !link.choices.iter().any(|choice| choice.id == light_id) {
        return Err("Choose a light from the Hue Bridge.".into());
    }
    let (device, state, model) = hue::connect(link.bridge.clone(), light_id).await?;
    save(Device::Hue(device), model, state, None)
}

#[tauri::command]
pub async fn lan_light_state(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<LampState, String> {
    check_window(&window)?;
    let _guard = controller.0.lock().await;
    let profile = read(&id)?;
    let mut state = match &profile.device {
        Device::Yeelight(device) => yeelight::state(device).await?,
        Device::Hue(device) => hue::state(device).await?,
    };
    state.name = profile.name;
    Ok(state)
}

#[tauri::command]
pub async fn lan_light_update(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
    update: LampUpdate,
) -> Result<LampState, String> {
    check_window(&window)?;
    update_saved(&controller, &id, update).await
}

pub(crate) async fn sunset_identity(controller: &Controller, id: &str) -> Result<(), String> {
    let _guard = controller.0.lock().await;
    read(id)?;
    Ok(())
}

pub(crate) async fn update_saved(
    controller: &Controller,
    id: &str,
    update: LampUpdate,
) -> Result<LampState, String> {
    let _guard = controller.0.lock().await;
    let profile = read(id)?;
    let mut state = match &profile.device {
        Device::Yeelight(device) => yeelight::update(device, update).await?,
        Device::Hue(device) => hue::update(device, update).await?,
    };
    state.name = profile.name;
    Ok(state)
}

#[tauri::command]
pub async fn lan_light_disconnect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<(), String> {
    check_window(&window)?;
    {
        let _guard = controller.0.lock().await;
        credentials::remove(&target(&id)?)?;
    }
    if let Some((provider, _)) = id.split_once('-') {
        crate::light_sunset::forget(&window, provider, &id).await;
    }
    Ok(())
}
