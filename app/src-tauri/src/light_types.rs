use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

#[derive(Serialize)]
pub(crate) struct DiscoveryResult {
    pub ip: Ipv4Addr,
    pub model: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LampState {
    pub name: String,
    pub on: bool,
    pub brightness: u8,
    pub hue: u16,
    pub saturation: u8,
    pub color_temperature: u16,
    pub supports_brightness: bool,
    pub supports_color: bool,
    pub supports_temperature: bool,
    pub min_brightness: u8,
    pub min_color_temperature: u16,
    pub max_color_temperature: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LampUpdate {
    pub on: Option<bool>,
    pub brightness: Option<u8>,
    pub hue: Option<u16>,
    pub saturation: Option<u8>,
    pub color_temperature: Option<u16>,
}

impl LampUpdate {
    pub(crate) fn validate(&self, state: &LampState) -> Result<(), String> {
        if self.on.is_none()
            && self.brightness.is_none()
            && self.hue.is_none()
            && self.color_temperature.is_none()
            && self.saturation.is_none()
        {
            return Err("Choose a setting to change.".into());
        }
        if self.brightness.is_some_and(|value| {
            !state.supports_brightness || !(state.min_brightness..=100).contains(&value)
        }) {
            return Err("Choose an available brightness for this light.".into());
        }
        if self.hue.is_some() != self.saturation.is_some()
            || self.hue.is_some_and(|hue| {
                !state.supports_color || hue > 360 || self.color_temperature.is_some()
            })
            || self.saturation.is_some_and(|saturation| saturation > 100)
        {
            return Err("Choose an available color for this light.".into());
        }
        if self.color_temperature.is_some_and(|kelvin| {
            !state.supports_temperature
                || !(state.min_color_temperature..=state.max_color_temperature).contains(&kelvin)
        }) {
            return Err("Choose an available white temperature for this light.".into());
        }
        Ok(())
    }
}

pub(crate) fn local_ipv4(input: &str) -> Result<Ipv4Addr, String> {
    input
        .trim()
        .parse::<Ipv4Addr>()
        .ok()
        .filter(Ipv4Addr::is_private)
        .ok_or_else(|| "Enter the device’s private IPv4 address.".into())
}

pub(crate) fn device_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned()
}
