use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::light_types::{DiscoveryResult, LampState, LampUpdate};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::Sha256;
use tokio::{
    net::UdpSocket,
    time::{Instant, timeout_at},
};

const PORT: u16 = 38899;
const MAX_PACKET: usize = 8192;
const MAX_DISCOVERED: usize = 32;
const RESPONSE_WINDOW: Duration = Duration::from_millis(850);
const WRITE_RESPONSE_WINDOW: Duration = Duration::from_millis(1700);
const DISCOVERY_WINDOW: Duration = Duration::from_secs(2);
const GET_CONFIG: &str = "getSystemConfig";
const GET_PILOT: &str = "getPilot";
const SET_PILOT: &str = "setPilot";
const MISMATCH: &str =
    "This IP belongs to a different WiZ light. Pair the light again with its current IP.";
const UNREACHABLE: &str = "The WiZ light did not respond. Check its power, Wi-Fi, and IP address, and allow local communication in the WiZ app.";
const VERIFIED_CONTROL: &str = "WiZ refused the command. If Only verified controls is enabled, reconnect the light with your WiZ home security key.";
const CHECK_KEY: &str =
    "WiZ refused the signed command. Check the home security key and your PC's date and time.";

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Device {
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
    model: String,
    capabilities: Capabilities,
}

impl Device {
    pub(crate) fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Capabilities {
    brightness: bool,
    color: bool,
    min_brightness: u8,
    temperature: Option<(u16, u16)>,
}

impl LampUpdate {
    fn params(self, capabilities: Capabilities) -> Result<Map<String, Value>, String> {
        let mut params = Map::new();
        if let Some(on) = self.on {
            params.insert("state".into(), json!(on));
        }
        if let Some(brightness) = self.brightness {
            if !capabilities.brightness {
                return Err("This WiZ light does not support brightness control.".into());
            }
            if !(capabilities.min_brightness..=100).contains(&brightness) {
                return Err(format!(
                    "Choose a brightness from {}% to 100%.",
                    capabilities.min_brightness
                ));
            }
            params.insert("dimming".into(), json!(brightness));
        }
        if self.hue.is_some() != self.saturation.is_some()
            || self.hue.is_some_and(|hue| hue > 360)
            || self.saturation.is_some_and(|saturation| saturation > 100)
        {
            return Err("Choose both a hue from 0 to 360 and saturation from 0 to 100.".into());
        }
        if let (Some(hue), Some(saturation)) = (self.hue, self.saturation) {
            if self.color_temperature.is_some() {
                return Err("Choose either a color or a white temperature.".into());
            }
            if !capabilities.color {
                return Err("This WiZ light does not support color control.".into());
            }
            let [r, g, b] = rgb_from_hue(hue, saturation);
            params.insert("r".into(), json!(r));
            params.insert("g".into(), json!(g));
            params.insert("b".into(), json!(b));
        }
        if let Some(temperature) = self.color_temperature {
            let Some((min, max)) = capabilities.temperature else {
                return Err("This WiZ light does not support white temperature control.".into());
            };
            if !(min..=max).contains(&temperature) {
                return Err(format!("Choose a white temperature from {min} to {max} K."));
            }
            params.insert("temp".into(), json!(temperature));
        }
        if params.is_empty() {
            return Err("Choose a setting to change.".into());
        }
        Ok(params)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemConfig {
    mac: String,
    module_name: Option<String>,
    type_id: Option<u32>,
}

impl SystemConfig {
    fn light_identity(&self) -> Result<([u8; 6], &str), String> {
        let mac = parse_mac(&self.mac).ok_or("The WiZ light did not identify itself.")?;
        let model = match self.module_name.as_deref() {
            Some(name)
                if !name.is_empty()
                    && name.len() <= 64
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
            {
                if name.contains("SOCKET") || name.contains("PLUG") {
                    return Err("Choose a WiZ light. This device is a smart plug.".into());
                }
                name
            }
            // Early firmware used typeId instead of returning a module name.
            None if self.type_id == Some(0) => "WiZ light",
            _ => return Err("The device did not report a supported WiZ light model.".into()),
        };
        Ok((mac, model))
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelConfig {
    min_dim_level: Option<u8>,
    cct_range: Option<Vec<f64>>,
    ext_range: Option<Vec<f64>>,
}

impl Capabilities {
    fn from_config(name: &str, config: ModelConfig, pilot: &Pilot) -> Self {
        let color = name.contains("RGB") || pilot.rgb().is_some();
        let temperature = if color || name.contains("TW") {
            let mut temperatures = config
                .cct_range
                .or(config.ext_range)
                .unwrap_or_default()
                .into_iter()
                .filter(|value| (1000.0..=10000.0).contains(value))
                .map(|value| value.round() as u16);
            let range = temperatures.next().map(|first| {
                temperatures.fold((first, first), |(min, max), value| {
                    (min.min(value), max.max(value))
                })
            });
            Some(range.filter(|(min, max)| min < max).unwrap_or((2700, 6500)))
        } else {
            None
        };
        let min_brightness = config
            .min_dim_level
            .filter(|value| (1..=100).contains(value))
            .unwrap_or(10);
        Self {
            brightness: min_brightness < 100
                && (config.min_dim_level.is_some()
                    || pilot.dimming.is_some()
                    || color
                    || temperature.is_some()
                    || name.contains("DW")
                    || name.contains("FANDIM")
                    || name == "WiZ light"),
            color,
            min_brightness,
            temperature,
        }
    }
}

#[derive(Deserialize)]
struct Pilot {
    mac: Option<String>,
    state: bool,
    dimming: Option<u8>,
    r: Option<u8>,
    g: Option<u8>,
    b: Option<u8>,
    temp: Option<u16>,
}

impl Pilot {
    fn rgb(&self) -> Option<[u8; 3]> {
        let rgb = [self.r?, self.g?, self.b?];
        (rgb != [0, 0, 0]).then_some(rgb)
    }

    fn into_state(self, device: &Device) -> LampState {
        let capabilities = device.capabilities;
        let (min_color_temperature, max_color_temperature) =
            capabilities.temperature.unwrap_or((0, 0));
        let color_temperature = self
            .temp
            .filter(|temp| (1000..=10000).contains(temp))
            .unwrap_or(0);
        let (hue, saturation) = if color_temperature == 0 {
            self.rgb().map(hue_from_rgb).unwrap_or_default()
        } else {
            (0, 0)
        };
        LampState {
            name: "WiZ light".into(),
            on: self.state,
            brightness: self
                .dimming
                .filter(|value| (1..=100).contains(value))
                .unwrap_or(100),
            hue,
            saturation,
            color_temperature,
            supports_brightness: capabilities.brightness,
            supports_color: capabilities.color,
            supports_temperature: capabilities.temperature.is_some(),
            min_brightness: capabilities.min_brightness,
            min_color_temperature,
            max_color_temperature,
        }
    }
}

enum Reply {
    Result(Value),
    Refused(Option<i32>),
}

#[derive(Deserialize)]
struct Packet {
    method: String,
    result: Option<Value>,
    error: Option<DeviceError>,
}

#[derive(Deserialize)]
struct DeviceError {
    code: Option<i32>,
}

impl Packet {
    fn parse(data: &[u8], method: &str) -> Option<Reply> {
        if data.len() > MAX_PACKET {
            return None;
        }
        let packet: Self = serde_json::from_slice(data).ok()?;
        if packet.method != method {
            return None;
        }
        if let Some(error) = packet.error {
            return Some(Reply::Refused(error.code));
        }
        packet.result.filter(Value::is_object).map(Reply::Result)
    }
}

pub(crate) fn parse_ip(input: &str) -> Result<Ipv4Addr, String> {
    let ip = input
        .trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| "Enter the WiZ light's local IPv4 address.")?;
    if !ip.is_private() {
        return Err("Enter a private IPv4 address for your WiZ light.".into());
    }
    Ok(ip)
}

fn parse_mac(value: &str) -> Option<[u8; 6]> {
    if !value.is_ascii() || !matches!(value.len(), 12 | 17) {
        return None;
    }
    let normalized: String = value.chars().filter(|c| *c != ':' && *c != '-').collect();
    if normalized.len() != 12 {
        return None;
    }
    let mut mac = [0; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&normalized[index * 2..index * 2 + 2], 16).ok()?;
    }
    (mac != [0; 6] && mac[0] & 1 == 0).then_some(mac)
}

pub(crate) fn rgb_from_hue(hue: u16, saturation: u8) -> [u8; 3] {
    let sector = f64::from(hue % 360) / 60.0;
    let chroma = f64::from(saturation) / 100.0;
    let secondary = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let base = 1.0 - chroma;
    let rgb = match sector as u8 {
        0 => [chroma, secondary, 0.0],
        1 => [secondary, chroma, 0.0],
        2 => [0.0, chroma, secondary],
        3 => [0.0, secondary, chroma],
        4 => [secondary, 0.0, chroma],
        _ => [chroma, 0.0, secondary],
    };
    rgb.map(|channel| ((channel + base) * 255.0).round() as u8)
}

pub(crate) fn hue_from_rgb([r, g, b]: [u8; 3]) -> (u16, u8) {
    let [r, g, b] = [r, g, b].map(f64::from);
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    if delta == 0.0 {
        return (0, 0);
    }
    let sector = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (
        (sector * 60.0).round() as u16 % 360,
        (100.0 * delta / max).round() as u8,
    )
}

fn request_bytes(
    method: &str,
    mut params: Map<String, Value>,
    signing: Option<(&[u8], u64)>,
) -> Vec<u8> {
    if let Some((key, timestamp)) = signing {
        // WiZ signs exactly the compact params object, with the timestamp last.
        params.insert("sigTs".into(), json!(timestamp));
        let mut hmac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts every key length");
        hmac.update(Value::Object(params.clone()).to_string().as_bytes());
        let signature = STANDARD.encode(hmac.finalize().into_bytes());
        return json!({"method":method,"env":"pro","params":params,"hmac":signature})
            .to_string()
            .into_bytes();
    }
    json!({"method":method,"params":params})
        .to_string()
        .into_bytes()
}

async fn request(
    address: SocketAddrV4,
    method: &str,
    params: Map<String, Value>,
    key: Option<&[u8]>,
) -> Result<Reply, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|_| "Could not open a local connection for WiZ.")?;
    socket.connect(address).await.map_err(|_| UNREACHABLE)?;
    let mut buffer = [0; MAX_PACKET + 1];
    // Retrying reads handles dropped datagrams without restarting a light transition.
    let (attempts, response_window) = if method == SET_PILOT {
        (1, WRITE_RESPONSE_WINDOW)
    } else {
        (2, RESPONSE_WINDOW)
    };
    for _ in 0..attempts {
        let signing = if let Some(key) = key {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "Check your PC's date and time.")?
                .as_secs();
            Some((key, timestamp))
        } else {
            None
        };
        let message = request_bytes(method, params.clone(), signing);
        socket.send(&message).await.map_err(|_| UNREACHABLE)?;
        let deadline = Instant::now() + response_window;
        while let Ok(received) = timeout_at(deadline, socket.recv(&mut buffer)).await {
            if Instant::now() >= deadline {
                break;
            }
            let size = received.map_err(|_| UNREACHABLE)?;
            if let Some(reply) = Packet::parse(&buffer[..size], method) {
                return Ok(reply);
            }
        }
    }
    Err(UNREACHABLE.into())
}

impl Reply {
    fn require_result(self, key: Option<&[u8]>) -> Result<Value, String> {
        match self {
            Self::Result(value) => Ok(value),
            Self::Refused(Some(-32602 | -32605)) if key.is_some() => Err(CHECK_KEY.into()),
            Self::Refused(Some(-32602 | -32605)) => Err(VERIFIED_CONTROL.into()),
            Self::Refused(_) => Err("The WiZ light rejected the requested operation.".into()),
        }
    }
}

async fn system_config(address: SocketAddrV4, key: Option<&[u8]>) -> Result<SystemConfig, String> {
    let config = request(address, GET_CONFIG, Map::new(), key)
        .await?
        .require_result(key)?;
    serde_json::from_value(config)
        .map_err(|_| "The device returned an invalid WiZ configuration.".into())
}

async fn read_pilot(address: SocketAddrV4, key: Option<&[u8]>) -> Result<Pilot, String> {
    let reply = request(address, GET_PILOT, Map::new(), key)
        .await?
        .require_result(key)?;
    serde_json::from_value(reply).map_err(|_| "The WiZ light returned an invalid state.".into())
}

async fn model_config(address: SocketAddrV4, key: Option<&[u8]>) -> ModelConfig {
    match request(address, "getModelConfig", Map::new(), key).await {
        Ok(Reply::Result(config)) => serde_json::from_value(config).unwrap_or_default(),
        Ok(Reply::Refused(Some(-32601))) => {
            // Older bulbs report their extended range in getUserConfig.
            match request(address, "getUserConfig", Map::new(), key).await {
                Ok(Reply::Result(config)) => serde_json::from_value(config).unwrap_or_default(),
                _ => ModelConfig::default(),
            }
        }
        _ => ModelConfig::default(),
    }
}

async fn checked_pilot(
    address: SocketAddrV4,
    expected: [u8; 6],
    key: Option<&[u8]>,
) -> Result<Pilot, String> {
    let pilot = read_pilot(address, key).await?;
    let actual = if let Some(mac) = pilot.mac.as_deref().and_then(parse_mac) {
        mac
    } else {
        system_config(address, key).await?.light_identity()?.0
    };
    if actual != expected {
        return Err(MISMATCH.into());
    }
    Ok(pilot)
}

pub(crate) async fn connect(
    ip: Ipv4Addr,
    key: Option<&[u8]>,
) -> Result<(Device, LampState), String> {
    if !ip.is_private() {
        return Err("Enter a private IPv4 address for your WiZ light.".into());
    }
    connect_at(SocketAddrV4::new(ip, PORT), key).await
}

async fn connect_at(
    address: SocketAddrV4,
    key: Option<&[u8]>,
) -> Result<(Device, LampState), String> {
    let config = system_config(address, key).await?;
    let (mac, model) = config.light_identity()?;
    let pilot = read_pilot(address, key).await?;
    if pilot
        .mac
        .as_deref()
        .is_some_and(|value| parse_mac(value) != Some(mac))
    {
        return Err(MISMATCH.into());
    }
    let capabilities = Capabilities::from_config(model, model_config(address, key).await, &pilot);
    let device = Device {
        ip: *address.ip(),
        mac,
        model: model.into(),
        capabilities,
    };
    let state = pilot.into_state(&device);
    Ok((device, state))
}

fn device_address(device: &Device) -> Result<SocketAddrV4, String> {
    if !device.ip.is_private() {
        return Err("Reconnect the WiZ light with its private IPv4 address.".into());
    }
    Ok(SocketAddrV4::new(device.ip, PORT))
}

pub(crate) async fn state(device: &Device, key: Option<&[u8]>) -> Result<LampState, String> {
    let pilot = checked_pilot(device_address(device)?, device.mac, key).await?;
    Ok(pilot.into_state(device))
}

pub(crate) async fn update(
    device: &Device,
    change: LampUpdate,
    key: Option<&[u8]>,
) -> Result<LampState, String> {
    update_at(device_address(device)?, device, change, key).await
}

async fn update_at(
    address: SocketAddrV4,
    device: &Device,
    change: LampUpdate,
    key: Option<&[u8]>,
) -> Result<LampState, String> {
    let params = change.params(device.capabilities)?;
    checked_pilot(address, device.mac, key).await?;
    let reply = request(address, SET_PILOT, params, key)
        .await?
        .require_result(key)?;
    if reply["success"].as_bool() != Some(true) {
        return Err("The WiZ light did not confirm the change.".into());
    }
    let pilot = checked_pilot(address, device.mac, key).await?;
    Ok(pilot.into_state(device))
}

fn broadcasts() -> BTreeSet<Ipv4Addr> {
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

pub(crate) async fn discover() -> Result<Vec<DiscoveryResult>, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|_| "Could not start WiZ discovery.")?;
    socket
        .set_broadcast(true)
        .map_err(|_| "Could not start WiZ discovery.")?;
    let destinations = broadcasts();
    let message = request_bytes(GET_CONFIG, Map::new(), None);
    let mut buffer = [0; MAX_PACKET + 1];
    let mut lights = BTreeMap::new();
    for window in [RESPONSE_WINDOW, DISCOVERY_WINDOW - RESPONSE_WINDOW] {
        let mut sent = false;
        for address in &destinations {
            sent |= socket.send_to(&message, (*address, PORT)).await.is_ok();
        }
        if !sent {
            return Err("Could not broadcast WiZ discovery over the local network.".into());
        }
        let deadline = Instant::now() + window;
        while let Ok(received) = timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            if Instant::now() >= deadline || lights.len() >= MAX_DISCOVERED {
                break;
            }
            let Ok((size, SocketAddr::V4(source))) = received else {
                break;
            };
            if !source.ip().is_private() || source.port() != PORT {
                continue;
            }
            if let Some(Reply::Result(result)) = Packet::parse(&buffer[..size], GET_CONFIG)
                && let Ok(config) = serde_json::from_value::<SystemConfig>(result)
                && let Ok((mac, model)) = config.light_identity()
            {
                lights.entry(mac).or_insert_with(|| DiscoveryResult {
                    ip: *source.ip(),
                    model: model.to_owned(),
                });
            }
        }
        if lights.len() >= MAX_DISCOVERED {
            break;
        }
    }
    let mut lights: Vec<_> = lights.into_values().collect();
    lights.sort_by_key(|result| result.ip);
    Ok(lights)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_the_serialized_parameters_in_wire_order() {
        let key: [u8; 16] = std::array::from_fn(|index| index as u8);
        let params = json!({"state": true, "dimming": 35})
            .as_object()
            .unwrap()
            .clone();
        let request = request_bytes(SET_PILOT, params, Some((&key, 1_760_000_000)));
        assert_eq!(
            String::from_utf8(request).unwrap(),
            concat!(
                r#"{"method":"setPilot","env":"pro","params":{"state":true,"dimming":35,"sigTs":1760000000},"#,
                r#""hmac":"gboT33cKLaQ0iraJa63Xovpe6j/UPX6j/2WyXv1qFi8="}"#,
            )
        );
    }

    #[test]
    fn lamp_controls_follow_the_model_configuration() {
        let pilot: Pilot = serde_json::from_value(json!({"state": true, "dimming": 2})).unwrap();
        let config = ModelConfig {
            min_dim_level: Some(1),
            cct_range: Some(vec![2200.0, 2700.0, 6500.0, 6500.0]),
            ext_range: None,
        };
        let capabilities = Capabilities::from_config("ESP25_SHTW_01", config, &pilot);
        assert!(capabilities.brightness);
        assert!(!capabilities.color);
        assert_eq!(capabilities.min_brightness, 1);
        assert_eq!(capabilities.temperature, Some((2200, 6500)));
        let update: LampUpdate =
            serde_json::from_value(json!({"hue": 120, "saturation": 100})).unwrap();
        assert!(
            update
                .params(capabilities)
                .unwrap_err()
                .contains("does not support color")
        );
        let update: LampUpdate =
            serde_json::from_value(json!({"colorTemperature": 2200, "brightness": 1})).unwrap();
        assert_eq!(
            Value::Object(update.params(capabilities).unwrap()),
            json!({"dimming": 1, "temp": 2200})
        );
    }

    #[test]
    fn pairs_and_controls_a_wiz_light_through_udp_and_stops_after_an_identity_change() {
        tauri::async_runtime::block_on(async {
            let bulb = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let SocketAddr::V4(address) = bulb.local_addr().unwrap() else {
                unreachable!()
            };
            let mac = "a8bb50001122";
            let pilot = json!({"mac": mac, "state": true, "temp": 2700, "dimming": 30});
            let script = [
                (
                    GET_CONFIG,
                    json!({"mac": mac, "moduleName": "ESP25_SHRGB_01"}),
                ),
                (GET_PILOT, pilot.clone()),
                (
                    "getModelConfig",
                    json!({"minDimLevel": 10, "cctRange": [2200, 2700, 6500, 6500]}),
                ),
                (GET_PILOT, pilot),
                (SET_PILOT, json!({"success": true})),
                (
                    GET_PILOT,
                    json!({"mac": mac, "state": true, "r": 0, "g": 255, "b": 0, "dimming": 35}),
                ),
                (
                    GET_PILOT,
                    json!({"mac": "a8bb50009988", "state": true, "dimming": 35}),
                ),
            ];
            let server = tokio::spawn(async move {
                let mut bytes = [0; 512];
                for (method, result) in script {
                    let (size, peer) = bulb.recv_from(&mut bytes).await.unwrap();
                    let request: Value = serde_json::from_slice(&bytes[..size]).unwrap();
                    assert_eq!(request["method"], method);
                    assert_eq!(request["env"], "pro");
                    assert!(request["params"]["sigTs"].is_u64());
                    assert!(request["hmac"].is_string());
                    if method == SET_PILOT {
                        assert_eq!(&request["params"]["dimming"], &json!(35));
                        assert_eq!(&request["params"]["g"], &json!(255));
                    }
                    let response = json!({"method": method, "result": result}).to_string();
                    bulb.send_to(response.as_bytes(), peer).await.unwrap();
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), bulb.recv_from(&mut bytes))
                        .await
                        .is_err()
                );
            });
            let key = Some(&[0x11; 16][..]);
            let (device, first) = connect_at(address, key).await.unwrap();
            assert_eq!(first.brightness, 30);
            assert_eq!(first.color_temperature, 2700);
            assert!(first.supports_color);
            assert_eq!(device.model(), "ESP25_SHRGB_01");
            let change =
                serde_json::from_value(json!({"brightness": 35, "hue": 120, "saturation": 100}))
                    .unwrap();
            let state = update_at(address, &device, change, key).await.unwrap();
            assert_eq!(state.brightness, 35);
            assert_eq!(state.hue, 120);
            let change = serde_json::from_value(json!({"on": false})).unwrap();
            assert_eq!(
                update_at(address, &device, change, key).await.unwrap_err(),
                MISMATCH
            );
            server.await.unwrap();
        });
    }
}
