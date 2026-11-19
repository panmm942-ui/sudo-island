//! Tapo bulbs are saved one connection per bulb, so any number can be paired.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
    net::Ipv4Addr,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tapo::responses::DeviceInfoColorLightResult;
use tapo::{ApiClient, ColorLightHandler, Error as TapoError, TapoResponseError};
use tokio::{sync::Mutex, time::timeout};
use tokio_stream::StreamExt;
use zeroize::{Zeroize, Zeroizing};

use crate::extension_net::credentials;
use crate::light_types::{LampState, LampUpdate};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(16);
const UNREACHABLE: &str = "The lamp did not respond. Check its power, Wi-Fi and local IP address.";
const NOT_CONNECTED: &str = "This Tapo lamp is no longer saved. Add it again in Tools → Light.";
const NOT_FOUND: &str = "Could not find your paired lamp. Check its power and Wi-Fi, or disconnect and pair again with its current IP.";
const UNREADABLE: &str =
    "The saved Tapo connection is unreadable. Disconnect and reconnect the lamp.";
/// The first lamp paired by releases that stored a single Tapo connection.
const LEGACY_ID: &str = "tapo";
const LEGACY_TARGET: &str = "SudoIsland/Tapo/Lamp";
const ID_PREFIX: &str = "tapo-";
const TARGET_PREFIX: &str = "SudoIsland/Tapo/Light/";
/// Windows Credential Manager's limit for one generic credential.
const BLOB_LIMIT: usize = 5 * 512;
type LampMac = [u8; 6];

#[derive(Default)]
pub struct Controller(Mutex<HashMap<String, LampSession>>);

#[derive(Serialize, Deserialize)]
struct Profile {
    ip: Ipv4Addr,
    email: String,
    password: String,
    device_id: String,
    #[serde(default)]
    mac: Option<LampMac>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
}

impl Drop for Profile {
    fn drop(&mut self) {
        self.password.zeroize();
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

#[derive(Serialize)]
pub struct DiscoveryResult {
    ip: Ipv4Addr,
    model: &'static str,
    /// Already saved in Sudo Island.
    paired: bool,
}

fn validate(update: &LampUpdate) -> Result<(), String> {
    if update.on.is_none()
        && update.brightness.is_none()
        && update.hue.is_none()
        && update.saturation.is_none()
        && update.color_temperature.is_none()
    {
        return Err("Choose a setting to change.".into());
    }
    if update
        .brightness
        .is_some_and(|value| !(1..=100).contains(&value))
    {
        return Err("Brightness must be between 1 and 100.".into());
    }
    if update.hue.is_some_and(|value| value > 360)
        || update
            .saturation
            .is_some_and(|value| !(1..=100).contains(&value))
        || update.hue.is_some() != update.saturation.is_some()
    {
        return Err("Choose both a hue between 0 and 360 and saturation between 1 and 100.".into());
    }
    if update
        .color_temperature
        .is_some_and(|value| !(2500..=6500).contains(&value))
    {
        return Err("White temperature must be between 2500 and 6500 K.".into());
    }
    if update.hue.is_some() && update.color_temperature.is_some() {
        return Err("Choose a color or a white temperature for each change.".into());
    }
    Ok(())
}

struct LampSession {
    handler: ColorLightHandler,
    device_id: String,
    mac: Option<LampMac>,
    model: &'static str,
}

struct DiscoveryReply {
    ip: Ipv4Addr,
    model: Option<&'static str>,
    mac: Option<LampMac>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A stable identifier for a newly paired bulb: its MAC, else a digest of its device ID.
fn new_id(device_id: &str, mac: Option<LampMac>) -> String {
    let key = mac.map_or_else(
        || hex(&Sha256::digest(device_id.as_bytes())[..6]),
        |mac| hex(&mac),
    );
    format!("{ID_PREFIX}{key}")
}

pub(crate) fn valid_id(id: &str) -> bool {
    id == LEGACY_ID
        || id.strip_prefix(ID_PREFIX).is_some_and(|key| {
            key.len() == 12
                && key
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

fn target(id: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("Invalid Tapo lamp identifier.".into());
    }
    Ok(match id.strip_prefix(ID_PREFIX) {
        Some(key) => format!("{TARGET_PREFIX}{key}"),
        None => LEGACY_TARGET.into(),
    })
}

fn read_profile(id: &str) -> Result<Option<Profile>, String> {
    let Some(blob) = credentials::read(&target(id)?)?.map(Zeroizing::new) else {
        return Ok(None);
    };
    serde_json::from_str(blob.as_str())
        .map(Some)
        .map_err(|_| UNREADABLE.into())
}

fn write_profile(id: &str, profile: &Profile) -> Result<(), String> {
    let blob = Zeroizing::new(
        serde_json::to_string(profile).map_err(|_| "Could not encode the Tapo connection.")?,
    );
    if blob.len() > BLOB_LIMIT {
        return Err("The Tapo connection exceeds the Windows Credential Manager limit.".into());
    }
    credentials::write_labeled(&target(id)?, "Sudo Island Tapo lamp", blob.as_str())
}

fn saved_ids() -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    if credentials::read(LEGACY_TARGET)?.map(Zeroizing::new).is_some() {
        ids.push(LEGACY_ID.to_owned());
    }
    for target in credentials::targets(TARGET_PREFIX)? {
        if let Some(key) = target.strip_prefix(TARGET_PREFIX) {
            let id = format!("{ID_PREFIX}{key}");
            if valid_id(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

/// Readable saved bulbs. One damaged entry must not hide the others.
fn saved_profiles() -> Result<Vec<(String, Profile)>, String> {
    Ok(saved_ids()?
        .into_iter()
        .filter_map(|id| read_profile(&id).ok().flatten().map(|profile| (id, profile)))
        .collect())
}

impl Profile {
    fn summary(&self, id: &str) -> SavedLight {
        let model = if self.model.is_empty() {
            "L530E"
        } else {
            self.model.as_str()
        };
        SavedLight {
            id: id.to_owned(),
            name: if self.name.trim().is_empty() {
                format!("Tapo {model}")
            } else {
                self.name.clone()
            },
            ip: self.ip,
            model: model.to_owned(),
        }
    }
}

fn valid_account(email: &str, password: &str) -> bool {
    email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(name, domain)| {
            !name.is_empty() && !domain.is_empty() && !domain.contains('@')
        })
        && !password.is_empty()
        && password.len() <= 512
        && !password.chars().any(char::is_control)
}

fn parse_mac(value: &str) -> Option<LampMac> {
    let mut digits = value.trim().chars().filter(|ch| !matches!(ch, ':' | '-'));
    let mut mac = [0; 6];
    for octet in &mut mac {
        let upper = digits.next()?.to_digit(16)?;
        let lower = digits.next()?.to_digit(16)?;
        *octet = ((upper << 4) | lower) as u8;
    }
    (digits.next().is_none() && mac != [0; 6] && mac[0] & 1 == 0).then_some(mac)
}

fn matches_paired(reply: &DiscoveryReply, mac: LampMac) -> bool {
    reply.model.is_some() && reply.mac == Some(mac)
}

fn select_paired_address(
    replies: &[DiscoveryReply],
    saved_ip: Ipv4Addr,
    mac: LampMac,
) -> Result<Ipv4Addr, String> {
    if let Some(ip) = replies
        .iter()
        .find(|reply| matches_paired(reply, mac))
        .map(|reply| reply.ip)
    {
        if replies
            .iter()
            .any(|reply| (reply.ip == ip) != matches_paired(reply, mac))
        {
            return Err("Several responses matched the paired lamp. Disconnect and pair again with its current IP.".into());
        }
        return Ok(ip);
    }

    // Discovery may be blocked while the old IP still works. An absent reply
    // lets us check the previously paired address with exact authenticated ID.
    // A contradictory reply rules that address out before authentication.
    if replies.iter().any(|reply| reply.ip == saved_ip) {
        return Err(NOT_FOUND.into());
    }
    Ok(saved_ip)
}

/// Color bulbs that share the Tapo color-light protocol.
const MODELS: [&str; 7] = ["L530E", "L530B", "L530", "L535E", "L535B", "L535", "L630"];

fn lamp_model(value: &str) -> Option<&'static str> {
    let model = value
        .split(|c: char| c == '(' || c.is_whitespace())
        .find(|part| !part.is_empty())?;
    MODELS
        .into_iter()
        .find(|known| model.eq_ignore_ascii_case(known))
}

fn private_ip(input: &str) -> Result<Ipv4Addr, String> {
    input
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
        .filter(Ipv4Addr::is_private)
        .ok_or_else(|| "Enter the lamp’s private IPv4 address.".into())
}

fn island_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "island" {
        Ok(())
    } else {
        Err("Open the Tapo controls from Tools → Light.".into())
    }
}

fn lamp_error(error: TapoError) -> String {
    match error {
        TapoError::UnsupportedProtocol {
            protocol: "AES", ..
        } => "Update the bulb’s firmware in the Tapo app to connect it to Sudo Island.",
        TapoError::UnsupportedProtocol { .. } => {
            "The lamp’s connection protocol is unsupported. Check its firmware in the Tapo app."
        }
        TapoError::Tapo(TapoResponseError::Unauthorized {
            kind: "TPAP_AUTH_ATTEMPTS_LIMIT",
            ..
        }) => "The lamp has temporarily locked sign-in. Wait before trying again.",
        TapoError::Tapo(TapoResponseError::Unauthorized {
            kind: "HASH_MISMATCH" | "TPAP_HASH_MISMATCH" | "TPAP_CREDENTIALS" | "LOGIN",
            ..
        }) => "Sign-in failed. Use the TP-Link account that set up this lamp.",
        TapoError::Tapo(TapoResponseError::Unauthorized {
            kind: "FORBIDDEN", ..
        }) => "The lamp refused local sign-in. Check the bulb’s firmware and Tapo app settings.",
        _ => UNREACHABLE,
    }
    .into()
}

fn session_expired(error: &TapoError) -> bool {
    matches!(
        error,
        TapoError::Tapo(TapoResponseError::Unauthorized {
            kind: "SESSION_TIMEOUT" | "SESSION_EXPIRED" | "TPAP_SESSION_INVALID",
            ..
        }) | TapoError::Tapo(TapoResponseError::DeviceError {
            kind: "TPAP_SESSION_INVALID",
            ..
        })
    )
}

fn verify_device(
    info: &DeviceInfoColorLightResult,
    device_id: &str,
    mac: Option<LampMac>,
) -> Result<&'static str, String> {
    let model = lamp_model(&info.model)
        .filter(|_| info.r#type == "SMART.TAPOBULB" && !info.device_id.is_empty())
        .ok_or("This address did not identify a supported Tapo color bulb (L530, L535 or L630).")?;
    if (!device_id.is_empty() && device_id != info.device_id)
        || mac.is_some_and(|expected| parse_mac(&info.mac) != Some(expected))
    {
        return Err("The saved IP belongs to a different bulb. Reconnect your Tapo lamp.".into());
    }
    Ok(model)
}

fn lamp_state(info: DeviceInfoColorLightResult, model: &str) -> LampState {
    let name = crate::light_types::device_name(&info.nickname);
    LampState {
        name: if name.is_empty() {
            format!("Tapo {model}")
        } else {
            name
        },
        on: info.device_on,
        brightness: info.brightness.clamp(1, 100),
        hue: info.hue.unwrap_or(0).min(360),
        saturation: info.saturation.unwrap_or(0).min(100) as u8,
        color_temperature: info.color_temp,
        supports_brightness: true,
        supports_color: true,
        supports_temperature: true,
        min_brightness: 1,
        min_color_temperature: 2500,
        max_color_temperature: 6500,
    }
}

impl LampSession {
    async fn open(profile: &Profile) -> Result<(Self, LampState), String> {
        let ip = private_ip(&profile.ip.to_string())?;
        let handler = ApiClient::new(profile.email.clone(), profile.password.clone())
            .with_timeout(REQUEST_TIMEOUT)
            .l530(ip.to_string())
            .await
            .map_err(lamp_error)?;
        let info = handler.get_device_info().await.map_err(lamp_error)?;
        let model = verify_device(&info, &profile.device_id, profile.mac)?;
        let session = Self {
            handler,
            device_id: info.device_id.clone(),
            mac: parse_mac(&info.mac),
            model,
        };
        Ok((session, lamp_state(info, model)))
    }

    async fn send_update(&self, update: &LampUpdate) -> Result<(), TapoError> {
        let mut request = self.handler.set();
        if let Some(on) = update.on {
            request = if on { request.on() } else { request.off() };
        }
        if let Some(brightness) = update.brightness {
            request = request.brightness(brightness);
        }
        if let (Some(hue), Some(saturation)) = (update.hue, update.saturation) {
            request = request.hue_saturation(hue, saturation);
        }
        if let Some(temperature) = update.color_temperature {
            request = request.color_temperature(temperature);
        }
        request.send(&self.handler).await
    }
}

async fn find_paired_address(ip: Ipv4Addr, mac: LampMac) -> Result<Ipv4Addr, String> {
    let mut replies = discover_at(ip, 1).await.unwrap_or_default();
    if replies
        .iter()
        .any(|reply| reply.ip == ip && matches_paired(reply, mac))
    {
        return Ok(ip);
    }

    // UDP discovery device_id is a different identifier from get_device_info.device_id.
    // Match the MAC only to select an address. The authenticated device ID verifies it.
    replies.extend(discover_lamps().await.unwrap_or_default());
    select_paired_address(&replies, ip, mac)
}

async fn restore_saved_session(id: &str) -> Result<(LampSession, LampState), String> {
    let mut profile = read_profile(id)?.ok_or(NOT_CONNECTED)?;
    if profile.device_id.is_empty() || !profile.ip.is_private() {
        return Err("The paired lamp identity is unreadable. Disconnect and pair again.".into());
    }
    let old_ip = profile.ip;
    if let Some(mac) = profile.mac {
        profile.ip = find_paired_address(old_ip, mac).await?;
    }

    let (session, state) = LampSession::open(&profile).await?;
    let moved = profile.ip != old_ip;
    remember(id, &mut profile, &session, &state, moved)?;
    Ok((session, state))
}

/// Keeps the saved address, name and model current, so the list of lights reads without the network.
fn remember(
    id: &str,
    profile: &mut Profile,
    session: &LampSession,
    state: &LampState,
    moved: bool,
) -> Result<(), String> {
    if !moved
        && session.mac == profile.mac
        && profile.name == state.name
        && profile.model == session.model
    {
        return Ok(());
    }
    profile.mac = session.mac;
    profile.name.clone_from(&state.name);
    profile.model = session.model.to_owned();
    write_profile(id, profile)
}

type Sessions = HashMap<String, LampSession>;

async fn take_session(
    active: &mut Sessions,
    id: &str,
) -> Result<(LampSession, Option<LampState>), String> {
    if let Some(session) = active.remove(id) {
        return Ok((session, None));
    }
    let (session, state) = restore_saved_session(id).await?;
    Ok((session, Some(state)))
}

async fn read_or_restore(
    id: &str,
    session: LampSession,
) -> Result<(LampSession, LampState), String> {
    let info = match session.handler.get_device_info().await {
        Err(error)
            if session_expired(&error)
                || matches!(&error, TapoError::Http(_) | TapoError::DeviceNotFound) =>
        {
            return restore_saved_session(id).await;
        }
        result => result.map_err(lamp_error)?,
    };
    verify_device(&info, &session.device_id, session.mac)?;
    let state = lamp_state(info, session.model);
    if let Some(mut profile) = read_profile(id)? {
        remember(id, &mut profile, &session, &state, false)?;
    }
    Ok((session, state))
}

async fn bounded<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    timeout(COMMAND_TIMEOUT, future)
        .await
        .map_err(|_| UNREACHABLE.to_string())?
}

#[tauri::command]
pub async fn tapo_list(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Vec<SavedLight>, String> {
    island_window(&window)?;
    let _guard = controller.0.lock().await;
    let mut lights: Vec<_> = saved_ids()?
        .into_iter()
        .map(|id| match read_profile(&id) {
            Ok(Some(profile)) => profile.summary(&id),
            // Listed so an unreadable connection can still be disconnected.
            _ => SavedLight {
                name: "Tapo lamp".into(),
                ip: Ipv4Addr::UNSPECIFIED,
                model: String::new(),
                id,
            },
        })
        .collect();
    lights.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(lights)
}

/// The TP-Link account already saved with a paired lamp, offered when adding another.
#[tauri::command]
pub async fn tapo_account(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Option<String>, String> {
    island_window(&window)?;
    let _guard = controller.0.lock().await;
    Ok(saved_profiles()?
        .into_iter()
        .find(|(_, profile)| valid_account(&profile.email, &profile.password))
        .map(|(_, profile)| profile.email.clone()))
}

fn broadcast_addresses() -> BTreeSet<Ipv4Addr> {
    let mut addresses = BTreeSet::from([Ipv4Addr::BROADCAST]);
    for interface in if_addrs::get_if_addrs().unwrap_or_default() {
        if !interface.is_oper_up() || interface.is_p2p() {
            continue;
        }
        if let if_addrs::IfAddr::V4(address) = interface.addr
            && address.ip.is_private()
            && let Some(broadcast) = address.broadcast.filter(Ipv4Addr::is_private)
        {
            addresses.insert(broadcast);
        }
        if addresses.len() == 12 {
            break;
        }
    }
    addresses
}

async fn discover_at(address: Ipv4Addr, timeout_seconds: u64) -> Result<Vec<DiscoveryReply>, ()> {
    let mut replies = ApiClient::discover_devices_raw(address.to_string(), timeout_seconds)
        .await
        .map_err(|_| ())?;
    let mut found = Vec::new();
    while let Some(reply) = replies.next().await {
        let Ok(reply) = reply else { continue };
        let std::net::IpAddr::V4(ip) = reply.ip else {
            continue;
        };
        let device = &reply.message["result"];
        let Some(device_type) = device["device_type"].as_str() else {
            continue;
        };
        if ip.is_private() {
            let model = if device_type == "SMART.TAPOBULB" {
                device["device_model"].as_str().and_then(lamp_model)
            } else {
                None
            };
            let mac = model.and_then(|_| device["mac"].as_str().and_then(parse_mac));
            found.push(DiscoveryReply { ip, model, mac });
        }
    }
    Ok(found)
}

async fn discover_lamps() -> Result<Vec<DiscoveryReply>, String> {
    let mut scans = tokio::task::JoinSet::new();
    for address in broadcast_addresses() {
        scans.spawn(discover_at(address, 3));
    }
    let mut found = Vec::new();
    let mut successful = false;
    while let Some(response) = scans.join_next().await {
        if let Ok(Ok(matches)) = response {
            successful = true;
            found.extend(matches);
        }
    }
    if !successful {
        return Err("Could not start Tapo discovery over the local network.".into());
    }
    Ok(found)
}

#[tauri::command]
pub async fn tapo_discover(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
) -> Result<Vec<DiscoveryResult>, String> {
    island_window(&window)?;
    bounded(async {
        let found = discover_lamps().await?;
        let _guard = controller.0.lock().await;
        let saved = saved_profiles()?;
        let unique: BTreeMap<_, _> = found
            .into_iter()
            .filter_map(|reply| {
                let paired = saved
                    .iter()
                    .any(|(_, profile)| match (profile.mac, reply.mac) {
                        (Some(saved), Some(found)) => saved == found,
                        _ => profile.ip == reply.ip,
                    });
                reply.model.map(|model| (reply.ip, (model, paired)))
            })
            .collect();
        Ok(unique
            .into_iter()
            .map(|(ip, (model, paired))| DiscoveryResult { ip, model, paired })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn tapo_connect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    ip: String,
    email: Option<String>,
    password: Option<String>,
) -> Result<ConnectedLight, String> {
    let password = Zeroizing::new(password.unwrap_or_default());
    island_window(&window)?;
    let mut profile = Profile {
        ip: private_ip(&ip)?,
        email: email.unwrap_or_default().trim().to_owned(),
        password: password.as_str().to_owned(),
        device_id: String::new(),
        mac: None,
        name: String::new(),
        model: String::new(),
    };
    // Adding another bulb can reuse the TP-Link account saved with the first.
    let reuse_account = profile.email.is_empty() && profile.password.is_empty();
    if !reuse_account && !valid_account(&profile.email, &profile.password) {
        return Err("Enter your TP-Link account email and password.".into());
    }
    bounded(async {
        let mut active = controller.0.lock().await;
        let saved = saved_profiles()?;
        if reuse_account {
            let (_, account) = saved
                .iter()
                .find(|(_, saved)| valid_account(&saved.email, &saved.password))
                .ok_or("Enter your TP-Link account email and password.")?;
            profile.email.clone_from(&account.email);
            profile.password.clone_from(&account.password);
        }
        let (session, state) = LampSession::open(&profile).await?;
        // Pairing a saved bulb again refreshes its connection in place.
        let id = saved
            .iter()
            .find(|(_, saved)| saved.device_id == session.device_id)
            .map_or_else(
                || new_id(&session.device_id, session.mac),
                |(id, _)| id.clone(),
            );
        profile.device_id.clone_from(&session.device_id);
        profile.mac = session.mac;
        profile.name.clone_from(&state.name);
        profile.model = session.model.to_owned();
        write_profile(&id, &profile)?;
        let device = profile.summary(&id);
        active.insert(id, session);
        Ok(ConnectedLight { device, state })
    })
    .await
}

#[tauri::command]
pub async fn tapo_state(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<LampState, String> {
    island_window(&window)?;
    target(&id)?;
    bounded(async {
        let mut active = controller.0.lock().await;
        let (session, initial_state) = take_session(&mut active, &id).await?;
        let (session, state) = match initial_state {
            Some(state) => (session, state),
            None => read_or_restore(&id, session).await?,
        };
        active.insert(id.clone(), session);
        Ok(state)
    })
    .await
}

#[tauri::command]
pub async fn tapo_update(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
    update: LampUpdate,
) -> Result<LampState, String> {
    island_window(&window)?;
    update_saved(&controller, &id, update, None).await
}

pub(crate) async fn sunset_identity(controller: &Controller, id: &str) -> Result<String, String> {
    let _guard = controller.0.lock().await;
    read_profile(id)?
        .map(|profile| profile.device_id.clone())
        .filter(|device_id| !device_id.is_empty())
        .ok_or_else(|| NOT_CONNECTED.into())
}

pub(crate) async fn turn_on_at_sunset(
    controller: &Controller,
    id: &str,
    expected: &str,
) -> Result<(), String> {
    let update = LampUpdate {
        on: Some(true),
        brightness: None,
        hue: None,
        saturation: None,
        color_temperature: None,
    };
    update_saved(controller, id, update, Some(expected))
        .await
        .map(|_| ())
}

async fn update_saved(
    controller: &Controller,
    id: &str,
    update: LampUpdate,
    expected: Option<&str>,
) -> Result<LampState, String> {
    validate(&update)?;
    target(id)?;
    bounded(async {
        let mut active = controller.0.lock().await;
        if let Some(expected) = expected {
            let profile = read_profile(id)?.ok_or(NOT_CONNECTED)?;
            if profile.device_id != expected {
                return Err("This Tapo schedule belongs to a previously paired light.".into());
            }
            if active
                .get(id)
                .is_some_and(|session| session.device_id != expected)
            {
                active.remove(id);
            }
        }
        let (mut session, _) = take_session(&mut active, id).await?;
        match session.send_update(&update).await {
            Err(error) if session_expired(&error) => {
                (session, _) = restore_saved_session(id).await?;
                session.send_update(&update).await.map_err(lamp_error)?;
            }
            result => result.map_err(lamp_error)?,
        }
        let (session, state) = read_or_restore(id, session).await?;
        active.insert(id.to_owned(), session);
        Ok(state)
    })
    .await
}

#[tauri::command]
pub async fn tapo_disconnect(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    id: String,
) -> Result<(), String> {
    island_window(&window)?;
    let saved = target(&id)?;
    bounded(async {
        let mut active = controller.0.lock().await;
        credentials::remove(&saved)?;
        active.remove(&id);
        Ok(())
    })
    .await?;
    crate::light_sunset::forget(&window, "tapo", &id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVED_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 10, 10);
    const NEW_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 10, 20);
    const PAIRED_MAC: LampMac = [0x24, 0x2f, 0x29, 1, 2, 3];
    const OTHER_MAC: LampMac = [0x24, 0x2f, 0x29, 4, 5, 6];

    fn reply(ip: Ipv4Addr, mac: LampMac) -> DiscoveryReply {
        DiscoveryReply {
            ip,
            model: Some("L530E"),
            mac: Some(mac),
        }
    }

    #[test]
    fn parses_the_mac_formats_used_in_discovery_and_device_info() {
        for value in ["24:2f:29:01:02:03", "24-2F-29-01-02-03", "242F29010203"] {
            assert_eq!(parse_mac(value), Some(PAIRED_MAC));
        }
        for value in [
            "",
            "00:00:00:00:00:00",
            "ff-ff-ff-ff-ff-ff",
            "25:2f:29:01:02:03",
            "242F2901020x",
        ] {
            assert_eq!(parse_mac(value), None);
        }
    }

    #[test]
    fn selects_only_the_paired_bulb_after_an_address_change() {
        let replies = [
            reply(SAVED_IP, OTHER_MAC),
            reply(NEW_IP, PAIRED_MAC),
            reply(NEW_IP, PAIRED_MAC),
        ];
        assert_eq!(
            select_paired_address(&replies, SAVED_IP, PAIRED_MAC),
            Ok(NEW_IP)
        );
    }

    #[test]
    fn rejects_ambiguous_addresses_and_conflicting_replies() {
        let duplicated = [reply(SAVED_IP, PAIRED_MAC), reply(NEW_IP, PAIRED_MAC)];
        let conflicting = [reply(NEW_IP, PAIRED_MAC), reply(NEW_IP, OTHER_MAC)];
        assert!(select_paired_address(&duplicated, SAVED_IP, PAIRED_MAC).is_err());
        assert!(select_paired_address(&conflicting, SAVED_IP, PAIRED_MAC).is_err());
    }

    #[test]
    fn falls_back_to_saved_ip_only_when_absent_from_discovery() {
        assert_eq!(
            select_paired_address(&[reply(NEW_IP, OTHER_MAC)], SAVED_IP, PAIRED_MAC),
            Ok(SAVED_IP)
        );
        let other_device = DiscoveryReply {
            ip: SAVED_IP,
            model: None,
            mac: Some(OTHER_MAC),
        };
        assert!(select_paired_address(&[other_device], SAVED_IP, PAIRED_MAC).is_err());
    }

    #[test]
    fn reads_saved_profiles_from_before_mac_and_name_tracking() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "ip": "192.168.10.10", "email": "fixture@example.com", "password": "fixture",
            "device_id": "fixture-lamp",
        }))
        .unwrap();
        assert_eq!(profile.ip, SAVED_IP);
        assert_eq!(profile.device_id, "fixture-lamp");
        assert_eq!(profile.mac, None);
        assert_eq!(profile.summary(LEGACY_ID).name, "Tapo L530E");
    }

    #[test]
    fn every_bulb_gets_its_own_saved_connection() {
        let first = new_id("fixture-one", Some(PAIRED_MAC));
        let second = new_id("fixture-two", Some(OTHER_MAC));
        assert_eq!(first, "tapo-242f29010203");
        assert_ne!(target(&first).unwrap(), target(&second).unwrap());
        assert_eq!(
            target(&first).unwrap(),
            format!("{TARGET_PREFIX}242f29010203")
        );
        // The lamp paired before several were supported keeps its connection.
        assert_eq!(target(LEGACY_ID).unwrap(), LEGACY_TARGET);
        // A bulb that reports no usable MAC is still stable and distinct.
        let hashed = new_id("fixture-one", None);
        assert!(valid_id(&hashed));
        assert_eq!(hashed, new_id("fixture-one", None));
        assert_ne!(hashed, new_id("fixture-two", None));
    }

    #[test]
    fn only_normalized_ids_address_saved_connections() {
        for invalid in [
            "",
            "tapo-",
            "tapo-242F29010203",
            "tapo-242f2901020",
            "tapo-../Lamp000",
            "wiz",
        ] {
            assert!(target(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn recognises_supported_color_bulbs() {
        assert_eq!(lamp_model("L530E(EU)"), Some("L530E"));
        assert_eq!(lamp_model("l535b"), Some("L535B"));
        assert_eq!(lamp_model("L630 Series"), Some("L630"));
        assert_eq!(lamp_model("P110"), None);
        assert_eq!(lamp_model(""), None);
    }

    #[test]
    fn accepts_only_complete_account_details() {
        assert!(valid_account("fixture@example.com", "fixture"));
        for (email, password) in [
            ("", ""),
            ("fixture@example.com", ""),
            ("fixture", "x"),
            ("a b@example.com", "x"),
        ] {
            assert!(!valid_account(email, password));
        }
    }
}
