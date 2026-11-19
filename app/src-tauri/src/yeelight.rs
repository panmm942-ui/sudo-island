//! Yeelight LAN Control, including compatible Xiaomi/Mi bulbs.
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, UdpSocket},
    time::{Instant, timeout, timeout_at},
};

use crate::light_types::{DiscoveryResult, LampState, LampUpdate, device_name, local_ipv4};
use crate::wiz::{hue_from_rgb, rgb_from_hue};

const DISCOVERY_PORT: u16 = 1982;
const CONTROL_PORT: u16 = 55443;
const MULTICAST: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), DISCOVERY_PORT);
const SEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n\r\n";
const UNAVAILABLE: &str = "The Xiaomi/Yeelight light did not respond. Check its Wi-Fi, IP and LAN Control setting in the Yeelight app.";
const UNSUPPORTED: &str =
    "Could not identify a compatible Xiaomi/Yeelight light. This light needs Yeelight LAN Control.";
const QUOTA: &str = "The light reached its LAN Control command limit. Wait a minute and try again.";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const FRAME_LIMIT: u64 = 16 * 1024;
const PROPERTIES: [&str; 8] = [
    "power",
    "bright",
    "ct",
    "rgb",
    "hue",
    "sat",
    "color_mode",
    "name",
];

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Device {
    pub id: String,
    pub ip: Ipv4Addr,
    pub model: String,
    bright: bool,
    temperature: bool,
    hsv: bool,
    rgb: bool,
}

fn identity(input: &str) -> Option<String> {
    let input = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))?;
    let number = u64::from_str_radix(input, 16).ok()?;
    (input.len() <= 16 && number != 0).then(|| format!("{number:016x}"))
}

fn advertisement(data: &[u8], sender: Ipv4Addr) -> Option<Device> {
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "HTTP/1.1 200 OK" || !sender.is_private() {
        return None;
    }
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim()))
        .collect();
    if headers
        .get("st")
        .is_some_and(|value| !value.eq_ignore_ascii_case("wifi_bulb"))
    {
        return None;
    }
    let location: SocketAddrV4 = headers
        .get("location")?
        .strip_prefix("yeelight://")?
        .parse()
        .ok()?;
    if *location.ip() != sender || location.port() != CONTROL_PORT {
        return None;
    }
    let model = *headers.get("model")?;
    if model.is_empty()
        || model.len() > 32
        || !model
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        return None;
    }
    let methods = headers.get("support")?;
    let supports = |name| {
        methods
            .split_ascii_whitespace()
            .any(|method| method == name)
    };
    if !supports("get_prop") || !supports("set_power") {
        return None;
    }
    Some(Device {
        id: identity(headers.get("id")?)?,
        ip: sender,
        model: model.to_owned(),
        bright: supports("set_bright"),
        temperature: supports("set_ct_abx"),
        hsv: supports("set_hsv"),
        rgb: supports("set_rgb"),
    })
}

async fn search(ip: Option<Ipv4Addr>) -> Result<Vec<Device>, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|_| UNAVAILABLE)?;
    let directed = if let Some(ip) = ip {
        socket.send_to(SEARCH, (ip, DISCOVERY_PORT)).await.is_ok()
    } else {
        false
    };
    if socket.send_to(SEARCH, MULTICAST).await.is_err() && !directed {
        return Err(UNAVAILABLE.into());
    }
    let mut devices = BTreeMap::new();
    let mut buffer = [0; 4096];
    let deadline = Instant::now() + Duration::from_millis(1800);
    while devices.len() < 32 {
        let Ok(Ok((length, SocketAddr::V4(source)))) =
            timeout_at(deadline, socket.recv_from(&mut buffer)).await
        else {
            break;
        };
        if Instant::now() >= deadline {
            break;
        }
        if ip.is_some_and(|ip| *source.ip() != ip) {
            continue;
        }
        if let Some(device) = advertisement(&buffer[..length], *source.ip()) {
            if ip.is_some() {
                return Ok(vec![device]);
            }
            devices.insert(device.ip, device);
        }
    }
    Ok(devices.into_values().collect())
}

pub(crate) async fn discover() -> Result<Vec<DiscoveryResult>, String> {
    Ok(search(None)
        .await?
        .into_iter()
        .map(|device| DiscoveryResult {
            ip: device.ip,
            model: format!("Xiaomi / Yeelight ({})", device.model),
        })
        .collect())
}

async fn identify(ip: Ipv4Addr) -> Result<Device, String> {
    local_ipv4(&ip.to_string())?;
    search(Some(ip))
        .await?
        .into_iter()
        .next()
        .ok_or(UNSUPPORTED.into())
}

struct Connection {
    socket: BufReader<TcpStream>,
    next_id: u32,
}

impl Connection {
    async fn open(address: SocketAddrV4) -> Result<Self, String> {
        let socket = timeout(COMMAND_TIMEOUT, TcpStream::connect(address))
            .await
            .map_err(|_| UNAVAILABLE)?
            .map_err(|_| UNAVAILABLE)?;
        Ok(Self {
            socket: BufReader::new(socket),
            next_id: 1,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Vec<Value>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"id":id,"method":method,"params":params}).to_string() + "\r\n";
        let exchange = async {
            self.socket
                .get_mut()
                .write_all(message.as_bytes())
                .await
                .map_err(|_| UNAVAILABLE)?;
            for _ in 0..16 {
                let mut frame = Vec::new();
                let length = (&mut self.socket)
                    .take(FRAME_LIMIT + 1)
                    .read_until(b'\n', &mut frame)
                    .await
                    .map_err(|_| UNAVAILABLE)?;
                if length == 0 || length as u64 > FRAME_LIMIT || !frame.ends_with(b"\r\n") {
                    return Err("The Xiaomi/Yeelight light returned an invalid response.".into());
                }
                let response: Value = serde_json::from_slice(&frame)
                    .map_err(|_| "The Xiaomi/Yeelight light returned an invalid response.")?;
                if response["id"].as_u64() != Some(u64::from(id)) {
                    continue;
                }
                if let Some(result) = response["result"].as_array() {
                    return Ok(result.clone());
                }
                if response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.to_ascii_lowercase().contains("quota"))
                {
                    return Err(QUOTA.into());
                }
                return Err("The Xiaomi/Yeelight light refused the change.".into());
            }
            Err(UNAVAILABLE.into())
        };
        timeout(COMMAND_TIMEOUT, exchange)
            .await
            .map_err(|_| UNAVAILABLE)?
    }

    async fn state(&mut self, device: &Device) -> Result<LampState, String> {
        let properties = self.request("get_prop", json!(PROPERTIES)).await?;
        parse_state(device, &properties)
    }
}

fn parse_state(device: &Device, properties: &[Value]) -> Result<LampState, String> {
    let values: Vec<_> = properties
        .iter()
        .map(|value| value.as_str().unwrap_or(""))
        .collect();
    let [power, brightness, ct, rgb, hue, sat, mode, name] = values.as_slice() else {
        return Err("The Xiaomi/Yeelight light returned an incomplete state.".into());
    };
    let on = match *power {
        "on" => true,
        "off" => false,
        _ => return Err("The Xiaomi/Yeelight light returned an invalid power state.".into()),
    };
    let (hue, saturation) = if *mode == "3" {
        (
            hue.parse::<u16>().unwrap_or(0) % 360,
            sat.parse::<u8>().unwrap_or(0).min(100),
        )
    } else {
        let rgb = rgb.parse::<u32>().unwrap_or(0).to_be_bytes();
        hue_from_rgb([rgb[1], rgb[2], rgb[3]])
    };
    let name = device_name(name);
    Ok(LampState {
        name: if name.is_empty() {
            format!("Yeelight {}", &device.id[12..])
        } else {
            name
        },
        on,
        brightness: brightness.parse::<u8>().unwrap_or(100).clamp(1, 100),
        hue,
        saturation,
        color_temperature: if *mode == "2" && device.temperature {
            ct.parse().unwrap_or(0)
        } else {
            0
        },
        supports_brightness: device.bright,
        supports_color: device.rgb || device.hsv,
        supports_temperature: device.temperature,
        min_brightness: 1,
        min_color_temperature: if device.temperature { 1700 } else { 0 },
        max_color_temperature: if device.temperature { 6500 } else { 0 },
    })
}

pub(crate) async fn connect(ip: Ipv4Addr) -> Result<(Device, LampState), String> {
    let device = identify(ip).await?;
    let state = Connection::open(SocketAddrV4::new(ip, CONTROL_PORT))
        .await?
        .state(&device)
        .await?;
    Ok((device, state))
}

async fn verified_connection(device: &Device) -> Result<(Device, Connection), String> {
    let actual = identify(device.ip).await?;
    if actual.id != device.id {
        return Err("This IP belongs to a different Xiaomi/Yeelight light. Add your light using its current IP.".into());
    }
    let connection = Connection::open(SocketAddrV4::new(actual.ip, CONTROL_PORT)).await?;
    Ok((actual, connection))
}

pub(crate) async fn state(device: &Device) -> Result<LampState, String> {
    let (actual, mut connection) = verified_connection(device).await?;
    connection.state(&actual).await
}

fn changes(
    device: &Device,
    change: LampUpdate,
    current: &LampState,
) -> Result<Vec<(&'static str, Value)>, String> {
    change.validate(current)?;
    let mut changes = Vec::new();
    if let Some(on) = change.on {
        changes.push((
            "set_power",
            json!([if on { "on" } else { "off" }, "sudden", 0]),
        ));
    }
    if let Some(level) = change.brightness {
        changes.push(("set_bright", json!([level, "sudden", 0])));
    }
    if let (Some(hue), Some(saturation)) = (change.hue, change.saturation) {
        if device.hsv {
            changes.push(("set_hsv", json!([hue % 360, saturation, "sudden", 0])));
        } else {
            let [r, g, b] = rgb_from_hue(hue, saturation);
            let color = u32::from_be_bytes([0, r, g, b]);
            changes.push(("set_rgb", json!([color, "sudden", 0])));
        }
    }
    if let Some(kelvin) = change.color_temperature {
        changes.push(("set_ct_abx", json!([kelvin, "sudden", 0])));
    }
    Ok(changes)
}

pub(crate) async fn update(device: &Device, change: LampUpdate) -> Result<LampState, String> {
    let (actual, mut connection) = verified_connection(device).await?;
    let before = connection.state(&actual).await?;
    for (method, params) in changes(&actual, change, &before)? {
        let result = connection.request(method, params).await?;
        if result.first().and_then(Value::as_str) != Some("ok") {
            return Err("The Xiaomi/Yeelight light did not confirm the change.".into());
        }
    }
    connection.state(&actual).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_control_skips_notifications_and_matches_the_request_id() {
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
                panic!("IPv4 required")
            };
            let fixture = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut frame = String::new();
                socket.read_line(&mut frame).await.unwrap();
                assert!(frame.ends_with("\r\n"));
                let message: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(
                    message,
                    json!({"id":1,"method":"set_bright","params":[75,"sudden",0]})
                );
                socket.get_mut().write_all(
                    b"{\"method\":\"props\",\"params\":{\"bright\":\"75\"}}\r\n{\"id\":2,\"result\":[\"inapplicable\"]}\r\n{\"id\":1,\"result\":[\"ok\"]}\r\n"
                ).await.unwrap();
            });
            let mut socket = Connection::open(address).await.unwrap();
            let result = socket
                .request("set_bright", json!([75, "sudden", 0]))
                .await
                .unwrap();
            assert_eq!(result, vec![json!("ok")]);
            fixture.await.unwrap();
        });
    }

    #[test]
    fn parses_discovery_and_uses_advertised_color_commands() {
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        let reply = b"HTTP/1.1 200 OK\r\nLocation: yeelight://192.168.1.5:55443\r\nid: 0x000000000000000a\r\nmodel: color\r\nsupport: get_prop set_power set_bright set_rgb\r\n\r\n";
        let bulb = advertisement(reply, ip).unwrap();
        assert_eq!(bulb.id, "000000000000000a");
        assert!(advertisement(reply, Ipv4Addr::new(192, 168, 1, 6)).is_none());
        let properties = json!(["on", "45", "2700", "255", "", "", "1", "Desk"]);
        let current = parse_state(&bulb, properties.as_array().unwrap()).unwrap();
        assert_eq!(
            (current.hue, current.saturation, current.brightness),
            (240, 100, 45)
        );
        let update = serde_json::from_value(json!({"hue":0,"saturation":100})).unwrap();
        assert_eq!(
            changes(&bulb, update, &current).unwrap(),
            vec![("set_rgb", json!([16711680, "sudden", 0]))]
        );
        let update = serde_json::from_value(json!({"colorTemperature":4000})).unwrap();
        assert!(changes(&bulb, update, &current).is_err());
    }
}
