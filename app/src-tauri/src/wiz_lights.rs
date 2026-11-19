//! WiZ connections are stored per bulb, independent of the paired Tapo lamp.
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use tokio::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

use crate::extension_net::credentials;
use crate::light_types::{DiscoveryResult, LampState, LampUpdate};
use crate::wiz::{self, Device};

const TARGET_PREFIX: &str = "SudoIsland/WiZ/Light/";
const MISSING: &str = "This WiZ light is no longer saved. Add it again in Tools → Light.";

#[derive(Default)]
pub struct Controller(Mutex<()>);

#[derive(Serialize, Deserialize)]
struct Profile {
    device: Device,
    name: String,
    signing_key: Option<String>,
}

impl Drop for Profile {
    fn drop(&mut self) {
        self.signing_key.zeroize();
    }
}

#[derive(Serialize)]
pub struct SavedLight {
    id: String,
    name: String,
    ip: Ipv4Addr,
    model: String,
}

#[derive(Serialize)]
pub struct ConnectedLight {
    device: SavedLight,
    state: LampState,
}

fn check_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "island" {
        Ok(())
    } else {
        Err("Open the WiZ controls from Tools → Light.".into())
    }
}

fn light_id(mac: [u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn target(id: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("Invalid WiZ light identifier.".into());
    }
    Ok(format!("{TARGET_PREFIX}{id}"))
}

fn decode_key(value: Option<&str>) -> Result<Option<Zeroizing<[u8; 16]>>, String> {
    let Some(key) = value.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(None);
    };
    let error = || {
        "Enter the 32-character udp_signing_key from the WiZ app’s local integration export."
            .to_owned()
    };
    if key.len() != 32 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(error());
    }
    let mut decoded = Zeroizing::new([0; 16]);
    for (index, output) in decoded.iter_mut().enumerate() {
        *output = u8::from_str_radix(&key[index * 2..index * 2 + 2], 16).map_err(|_| error())?;
    }
    Ok(Some(decoded))
}

fn read_profile(id: &str) -> Result<Option<Profile>, String> {
    let Some(blob) = credentials::read(&target(id)?)?.map(Zeroizing::new) else {
        return Ok(None);
    };
    let unreadable = "The saved WiZ connection is unreadable. Remove it and add the light again.";
    let profile: Profile = serde_json::from_str(blob.as_str()).map_err(|_| unreadable)?;
    if light_id(profile.device.mac) != id || wiz::parse_ip(&profile.device.ip.to_string()).is_err()
    {
        return Err(unreadable.into());
    }
    Ok(Some(profile))
}

impl Profile {
    fn summary(&self) -> SavedLight {
        SavedLight {
            id: light_id(self.device.mac),
            name: self.name.clone(),
            ip: self.device.ip,
            model: self.device.model().to_owned(),
        }
    }
}

#[tauri::command]
pub async fn wiz_list(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Vec<SavedLight>, String> {
    check_window(&window)?;
    let _guard = controller.0.lock().await;
    let mut lights = Vec::new();
    for target in credentials::targets(TARGET_PREFIX)? {
        if let Some(id) = target.strip_prefix(TARGET_PREFIX).filter(|id| valid_id(id)) {
            if let Some(profile) = read_profile(id)? {
                lights.push(profile.summary());
            }
        }
    }
    lights.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(lights)
}

#[tauri::command]
pub async fn wiz_discover(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Vec<DiscoveryResult>, String> {
    check_window(&window)?;
    let _guard = controller.0.lock().await;
    wiz::discover().await
}

#[tauri::command]
pub async fn wiz_connect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    ip: String,
    name: Option<String>,
    signing_key: Option<String>,
) -> Result<ConnectedLight, String> {
    let signing_key = Zeroizing::new(signing_key.unwrap_or_default());
    check_window(&window)?;
    let ip = wiz::parse_ip(&ip)?;
    let name = name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if name.is_some_and(|name| name.chars().count() > 48 || name.chars().any(char::is_control)) {
        return Err("Use a light name of up to 48 characters.".into());
    }
    let key = decode_key(Some(&signing_key))?;
    let _guard = controller.0.lock().await;
    let (device, mut state) = wiz::connect(ip, key.as_deref().map(<[u8; 16]>::as_slice)).await?;
    let id = light_id(device.mac);
    let mut previous = read_profile(&id)?;
    let profile = Profile {
        device,
        name: name
            .map(str::to_owned)
            .or_else(|| {
                previous
                    .as_mut()
                    .map(|profile| std::mem::take(&mut profile.name))
            })
            .unwrap_or_else(|| format!("WiZ light {}", id[8..].to_ascii_uppercase())),
        signing_key: if key.is_some() {
            Some(signing_key.trim().to_owned())
        } else {
            previous
                .as_mut()
                .and_then(|profile| profile.signing_key.take())
        },
    };
    let blob = Zeroizing::new(
        serde_json::to_string(&profile).map_err(|_| "Could not save the WiZ light.")?,
    );
    credentials::write_labeled(&target(&id)?, "Sudo Island WiZ light", blob.as_str())?;
    state.name.clone_from(&profile.name);
    Ok(ConnectedLight {
        device: profile.summary(),
        state,
    })
}

#[tauri::command]
pub async fn wiz_state(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<LampState, String> {
    check_window(&window)?;
    let _guard = controller.0.lock().await;
    let mut profile = read_profile(&id)?.ok_or(MISSING)?;
    let key = decode_key(profile.signing_key.as_deref())?;
    let mut state = wiz::state(&profile.device, key.as_deref().map(<[u8; 16]>::as_slice)).await?;
    state.name = std::mem::take(&mut profile.name);
    Ok(state)
}

#[tauri::command]
pub async fn wiz_update(
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
    read_profile(id)?.ok_or(MISSING)?;
    Ok(())
}

pub(crate) async fn update_saved(
    controller: &Controller,
    id: &str,
    update: LampUpdate,
) -> Result<LampState, String> {
    let _guard = controller.0.lock().await;
    let mut profile = read_profile(id)?.ok_or(MISSING)?;
    let key = decode_key(profile.signing_key.as_deref())?;
    let mut state = wiz::update(
        &profile.device,
        update,
        key.as_deref().map(<[u8; 16]>::as_slice),
    )
    .await?;
    state.name = std::mem::take(&mut profile.name);
    Ok(state)
}

#[tauri::command]
pub async fn wiz_disconnect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<(), String> {
    check_window(&window)?;
    {
        let _guard = controller.0.lock().await;
        credentials::remove(&target(&id)?)?;
    }
    crate::light_sunset::forget(&window, "wiz", &id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_normalized_wifi_bulb_ids_address_saved_connections() {
        let id = light_id([0x10, 0x02, 0xaf, 0x01, 0x11, 0x2b]);
        assert_eq!(target(&id).unwrap(), format!("{TARGET_PREFIX}1002af01112b"));
        for invalid in ["../../Tapo/Lamp", "1002af01112b*", "1002AF01112B", ""] {
            assert!(target(invalid).is_err());
        }
    }

    #[test]
    fn decodes_only_wiz_export_keys() {
        let key = decode_key(Some(" 00112233445566778899aAbBcCdDeEfF "))
            .unwrap()
            .unwrap();
        assert_eq!([key[0], key[1], key[10], key[15]], [0, 0x11, 0xaa, 0xff]);
        assert!(decode_key(Some("")).unwrap().is_none());
        assert!(decode_key(Some("00112233445566778899aabbccddeegg")).is_err());
        assert!(decode_key(Some("00112233445566778899aabbccddee")).is_err());
    }
}
