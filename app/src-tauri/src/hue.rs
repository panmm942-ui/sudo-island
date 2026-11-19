//! Philips Hue's documented local CLIP API over HTTPS.
use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use reqwest::{Client, Method, Response, redirect::Policy, tls::TlsInfo};
use rustls::{
    ClientConfig, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::light_types::{DiscoveryResult, LampState, LampUpdate, device_name, local_ipv4};

const UNAVAILABLE: &str =
    "Could not reach the Philips Hue Bridge. Check its power and local IP address.";
const BRIDGE_CHANGED: &str = "The Hue Bridge could not be verified. Check its IP and reconnect it using the physical link button.";
const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Bridge {
    pub ip: Ipv4Addr,
    pub id: String,
    certificate: [u8; 32],
    key: Zeroizing<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct LightChoice {
    pub id: u32,
    pub name: String,
    pub model: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Device {
    pub bridge: Bridge,
    pub number: u32,
    pub unique_id: String,
}

#[derive(Debug)]
struct BridgeCertificate {
    sha256: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for BridgeCertificate {
    fn verify_server_cert(
        &self,
        certificate: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if <[u8; 32]>::from(Sha256::digest(certificate.as_ref())) != self.sha256 {
            return Err(rustls::Error::General(
                "The Hue Bridge certificate changed.".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, sig, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, sig, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn client() -> reqwest::ClientBuilder {
    Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(4))
}

fn pinned_client(certificate: [u8; 32]) -> Result<Client, String> {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = BridgeCertificate {
        sha256: certificate,
        algorithms: provider.signature_verification_algorithms,
    };
    let tls = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|_| BRIDGE_CHANGED)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    client()
        .use_preconfigured_tls(tls)
        .build()
        .map_err(|_| BRIDGE_CHANGED.into())
}

async fn decode<T: DeserializeOwned>(mut response: Response) -> Result<T, String> {
    if !response.status().is_success() {
        return Err("The Hue Bridge rejected the request.".into());
    }
    let mut body = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| UNAVAILABLE)? {
        if body.len() + chunk.len() > BODY_LIMIT {
            return Err("The Hue Bridge response was too large.".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "The Hue Bridge returned an invalid response.".into())
}

#[derive(Deserialize)]
struct DiscoveryEntry {
    id: String,
    internalipaddress: String,
}

fn bridge_id(id: &str) -> Option<String> {
    (id.len() == 16 && id.bytes().all(|c| c.is_ascii_hexdigit())).then(|| id.to_ascii_lowercase())
}

pub(crate) async fn discover() -> Result<Vec<DiscoveryResult>, String> {
    let response = client()
        .build()
        .map_err(|_| UNAVAILABLE)?
        .get("https://discovery.meethue.com/")
        .send()
        .await
        .map_err(|_| "Hue discovery is unavailable. Enter your bridge IP from the Hue app.")?;
    let entries: Vec<DiscoveryEntry> = decode(response).await?;
    let mut bridges: Vec<_> = entries
        .into_iter()
        .filter(|entry| bridge_id(&entry.id).is_some())
        .filter_map(|entry| local_ipv4(&entry.internalipaddress).ok())
        .take(16)
        .map(|ip| DiscoveryResult {
            ip,
            model: "Philips Hue Bridge".into(),
        })
        .collect();
    bridges.sort_by_key(|bridge| bridge.ip);
    bridges.dedup_by_key(|bridge| bridge.ip);
    Ok(bridges)
}

async fn bridge_config(ip: Ipv4Addr) -> Result<(String, [u8; 32]), String> {
    // A one-time probe contains no credentials. Subsequent traffic uses this certificate
    // as its trust anchor, and registration requires the physical link button.
    let response = client()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .build()
        .map_err(|_| UNAVAILABLE)?
        .get(format!("https://{ip}/api/config"))
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    let certificate = response
        .extensions()
        .get::<TlsInfo>()
        .and_then(TlsInfo::peer_certificate)
        .map(|der| <[u8; 32]>::from(Sha256::digest(der)))
        .ok_or(BRIDGE_CHANGED)?;
    let body: Value = decode(response).await?;
    let identity = body["bridgeid"]
        .as_str()
        .and_then(bridge_id)
        .ok_or("This address did not identify a Philips Hue Bridge.")?;
    Ok((identity, certificate))
}

pub(crate) async fn link(
    ip: Ipv4Addr,
    existing: Option<&Bridge>,
) -> Result<(Bridge, Vec<LightChoice>), String> {
    local_ipv4(&ip.to_string())?;
    let (id, certificate) = bridge_config(ip).await?;
    if let Some(saved) = existing.filter(|saved| saved.id == id && saved.certificate == certificate)
    {
        let bridge = Bridge {
            ip,
            ..saved.clone()
        };
        if let Ok(choices) = lights(&bridge).await {
            return Ok((bridge, choices));
        }
    }
    let response = pinned_client(certificate)?
        .post(format!("https://{ip}/api"))
        .json(&json!({"devicetype":"sudo_island#windows"}))
        .send()
        .await
        .map_err(|_| BRIDGE_CHANGED)?;
    let result: Vec<LinkResult> = decode(response).await?;
    let mut key = None;
    for item in result {
        if let Some(success) = item.success {
            key = Some(success.username);
            break;
        }
        if item.error.is_some_and(|error| error.kind == 101) {
            return Err(
                "Press the link button on the Hue Bridge, then choose Connect bridge again.".into(),
            );
        }
    }
    let key = key
        .filter(|value| valid_key(value))
        .ok_or("The Hue Bridge did not issue an app key.")?;
    let bridge = Bridge {
        ip,
        id,
        certificate,
        key,
    };
    let choices = lights(&bridge).await?;
    Ok((bridge, choices))
}

#[derive(Deserialize)]
struct LinkResult {
    success: Option<LinkSuccess>,
    error: Option<HueError>,
}

#[derive(Deserialize)]
struct LinkSuccess {
    username: Zeroizing<String>,
}

#[derive(Deserialize)]
struct HueError {
    #[serde(rename = "type")]
    kind: u16,
}

fn valid_key(key: &str) -> bool {
    (16..=96).contains(&key.len())
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

async fn send<T: DeserializeOwned>(
    bridge: &Bridge,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<T, String> {
    local_ipv4(&bridge.ip.to_string())?;
    if !valid_key(&bridge.key) {
        return Err("The saved Hue key is invalid. Connect the bridge again.".into());
    }
    let url = Zeroizing::new(format!(
        "https://{}/api/{}/lights{path}",
        bridge.ip, &*bridge.key
    ));
    let mut request = pinned_client(bridge.certificate)?.request(method, url.as_str());
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|_| BRIDGE_CHANGED)?;
    decode(response).await
}

#[derive(Deserialize)]
struct HueLight {
    name: String,
    modelid: String,
    uniqueid: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    state: HueState,
    capabilities: Option<HueCapabilities>,
}

#[derive(Deserialize)]
struct HueState {
    on: bool,
    reachable: Option<bool>,
    bri: Option<u8>,
    hue: Option<u16>,
    sat: Option<u8>,
    ct: Option<u16>,
    colormode: Option<String>,
}

#[derive(Deserialize)]
struct HueCapabilities {
    control: Option<HueControl>,
}

#[derive(Deserialize)]
struct HueControl {
    ct: Option<HueTemperature>,
}

#[derive(Deserialize)]
struct HueTemperature {
    min: u16,
    max: u16,
}

fn light_identity(light: &HueLight) -> Option<&str> {
    if light.kind.to_ascii_lowercase().contains("plug") {
        return None;
    }
    light.uniqueid.as_deref().filter(|id| {
        (8..=100).contains(&id.len())
            && id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, b':' | b'-'))
    })
}

fn temperature_limits(light: &HueLight) -> Option<(u16, u16)> {
    light.state.ct?;
    let limits = light
        .capabilities
        .as_ref()
        .and_then(|caps| caps.control.as_ref())
        .and_then(|control| control.ct.as_ref())
        .filter(|range| 100 <= range.min && range.min < range.max && range.max <= 1000);
    Some(limits.map_or((153, 500), |range| (range.min, range.max)))
}

fn from_mired(mired: u16) -> u16 {
    (1_000_000.0 / f64::from(mired)).round() as u16
}

fn to_mired(kelvin: u16) -> u16 {
    (1_000_000.0 / f64::from(kelvin)).round() as u16
}

impl HueLight {
    fn display_name(&self) -> String {
        let name = device_name(&self.name);
        if name.is_empty() {
            "Hue light".into()
        } else {
            name
        }
    }

    fn state(&self) -> LampState {
        let ranges = temperature_limits(self);
        let (min, max) = ranges.map_or((0, 0), |(min, max)| (from_mired(max), from_mired(min)));
        LampState {
            name: self.display_name(),
            on: self.state.on,
            brightness: self
                .state
                .bri
                .map_or(100, |level| {
                    (f64::from(level) * 100.0 / 254.0).round() as u8
                })
                .clamp(1, 100),
            hue: self.state.hue.map_or(0, |hue| {
                (f64::from(hue) * 360.0 / 65535.0).round() as u16 % 360
            }),
            saturation: self
                .state
                .sat
                .map_or(0, |sat| (f64::from(sat) * 100.0 / 254.0).round() as u8)
                .min(100),
            color_temperature: if self.state.colormode.as_deref() == Some("ct") {
                self.state
                    .ct
                    .filter(|&value| value > 0)
                    .map_or(0, from_mired)
            } else {
                0
            },
            supports_brightness: self.state.bri.is_some(),
            supports_color: self.state.hue.is_some() && self.state.sat.is_some(),
            supports_temperature: ranges.is_some(),
            min_brightness: 1,
            min_color_temperature: min,
            max_color_temperature: max,
        }
    }
}

async fn lights(bridge: &Bridge) -> Result<Vec<LightChoice>, String> {
    let items: std::collections::BTreeMap<u32, HueLight> =
        send(bridge, Method::GET, "", None).await.map_err(
            |_| "Could not read the lights on this Hue Bridge. Check the bridge connection.",
        )?;
    Ok(items
        .into_iter()
        .filter(|(_, light)| light_identity(light).is_some())
        .map(|(id, light)| LightChoice {
            id,
            name: light.display_name(),
            model: device_name(&light.modelid),
        })
        .collect())
}

async fn read(bridge: &Bridge, number: u32) -> Result<HueLight, String> {
    send(bridge, Method::GET, &format!("/{number}"), None).await
}

pub(crate) async fn connect(
    bridge: Bridge,
    number: u32,
) -> Result<(Device, LampState, String), String> {
    let light = read(&bridge, number).await?;
    let unique_id = light_identity(&light)
        .ok_or("Choose a supported Philips Hue light.")?
        .to_owned();
    let model = device_name(&light.modelid);
    Ok((
        Device {
            bridge,
            number,
            unique_id,
        },
        light.state(),
        model,
    ))
}

async fn checked_light(device: &Device) -> Result<HueLight, String> {
    let light = read(&device.bridge, device.number).await?;
    if light_identity(&light) != Some(&device.unique_id) {
        return Err("This light changed on the Hue Bridge. Add it again to reconnect.".into());
    }
    if light.state.reachable == Some(false) {
        return Err("The Hue Bridge cannot reach this light. Check that it has power.".into());
    }
    Ok(light)
}

pub(crate) async fn state(device: &Device) -> Result<LampState, String> {
    Ok(checked_light(device).await?.state())
}

fn update_body(change: LampUpdate, light: &HueLight) -> Result<Value, String> {
    change.validate(&light.state())?;
    let mut payload = serde_json::Map::new();
    if let Some(on) = change.on {
        payload.insert("on".into(), json!(on));
    }
    if let Some(level) = change.brightness {
        payload.insert(
            "bri".into(),
            json!((f64::from(level) * 254.0 / 100.0).round() as u8),
        );
    }
    if let (Some(hue), Some(sat)) = (change.hue, change.saturation) {
        payload.insert(
            "hue".into(),
            json!((f64::from(hue % 360) * 65535.0 / 360.0).round() as u16),
        );
        payload.insert(
            "sat".into(),
            json!((f64::from(sat) * 254.0 / 100.0).round() as u8),
        );
    }
    if let Some(kelvin) = change.color_temperature {
        let (min, max) =
            temperature_limits(light).ok_or("This light does not support white temperature.")?;
        payload.insert("ct".into(), json!(to_mired(kelvin).clamp(min, max)));
    }
    Ok(Value::Object(payload))
}

pub(crate) async fn update(device: &Device, change: LampUpdate) -> Result<LampState, String> {
    let payload = update_body(change, &checked_light(device).await?)?;
    let response: Vec<Value> = send(
        &device.bridge,
        Method::PUT,
        &format!("/{}/state", device.number),
        Some(&payload),
    )
    .await?;
    if response.is_empty() || response.iter().any(|item| !item["success"].is_object()) {
        return Err("The Hue Bridge did not confirm the change.".into());
    }
    state(device).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_different_bridge_certificate_cannot_receive_the_saved_key() {
        let peer = CertificateDer::from(&b"paired bridge certificate"[..]);
        let verifier = BridgeCertificate {
            sha256: Sha256::digest(peer.as_ref()).into(),
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        };
        let address = ServerName::try_from("192.168.1.2").unwrap();
        let now = UnixTime::now();
        assert!(
            verifier
                .verify_server_cert(&peer, &[], &address, &[], now)
                .is_ok()
        );
        let replacement = CertificateDer::from(&b"replacement bridge certificate"[..]);
        assert!(
            verifier
                .verify_server_cert(&replacement, &[], &address, &[], now)
                .is_err()
        );
    }

    #[test]
    fn maps_hue_control_to_bridge_units_and_uses_reported_capabilities() {
        let light: HueLight = serde_json::from_value(json!({
            "name":"Desk","modelid":"LCT015","uniqueid":"00:17:88:01:01:01:01:01-0b","type":"Extended color light",
            "state":{"on":true,"bri":127,"hue":0,"sat":0,"ct":250,"colormode":"ct"},
            "capabilities":{"control":{"ct":{"min":200,"max":500}}}
        })).unwrap();
        let initial = light.state();
        assert_eq!(
            (
                initial.brightness,
                initial.color_temperature,
                initial.max_color_temperature
            ),
            (50, 4000, 5000)
        );
        let update =
            serde_json::from_value(json!({"brightness":50,"colorTemperature":4000})).unwrap();
        assert_eq!(
            update_body(update, &light).unwrap(),
            json!({"bri":127,"ct":250})
        );
        let update = serde_json::from_value(json!({"colorTemperature":6500})).unwrap();
        assert!(update_body(update, &light).is_err());
    }
}
