//! Native daily sunset scheduling for paired lights, including while the Island is collapsed.
use std::time::Duration;

use chrono::{DateTime, Days, Utc};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, Notify};
use zeroize::Zeroizing;

use crate::{
    extension_net::credentials, lan_lights, light_types::LampUpdate, sunrise_sunset, tapo,
    wiz_lights,
};

const PREFIX: &str = "SudoIsland/Sunset/";
const CHECK_INTERVAL: Duration = Duration::from_secs(20);
const CATCH_UP_SECONDS: i64 = 60 * 60;
const FORECAST_DAYS: u64 = 90;
const REFRESH_SECONDS: i64 = 84 * 24 * 60 * 60;
const RETRY_SECONDS: i64 = 15 * 60;

#[derive(Default)]
pub struct Controller {
    settings: Mutex<()>,
    provider_cooldown: Mutex<i64>,
    changed: Notify,
}

#[derive(Clone, Deserialize, Serialize)]
struct Light {
    provider: String,
    id: String,
}

impl Light {
    fn new(provider: String, id: String) -> Result<Self, String> {
        let hex = |s: &str, length| {
            s.len() == length
                && s.bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        };
        let valid = match provider.as_str() {
            "tapo" => tapo::valid_id(&id),
            "wiz" => hex(&id, 12),
            "yeelight" => id.strip_prefix("yeelight-").is_some_and(|s| hex(s, 16)),
            "hue" => id.strip_prefix("hue-").is_some_and(|s| hex(s, 64)),
            _ => false,
        };
        if !valid {
            return Err("Choose a saved light.".into());
        }
        Ok(Self { provider, id })
    }

    fn target(&self) -> String {
        format!("{PREFIX}{}/{}", self.provider, self.id)
    }

    async fn binding(&self, app: &tauri::AppHandle) -> Result<String, String> {
        match self.provider.as_str() {
            "tapo" => tapo::sunset_identity(&app.state(), &self.id).await,
            "wiz" => {
                wiz_lights::sunset_identity(&app.state(), &self.id).await?;
                Ok(self.id.clone())
            }
            _ => {
                lan_lights::sunset_identity(&app.state(), &self.id).await?;
                Ok(self.id.clone())
            }
        }
    }

    async fn turn_on(&self, app: &tauri::AppHandle, identity: &str) -> Result<(), String> {
        if self.provider == "tapo" {
            return tapo::turn_on_at_sunset(&app.state(), &self.id, identity).await;
        }
        let update = LampUpdate {
            on: Some(true),
            brightness: None,
            hue: None,
            saturation: None,
            color_temperature: None,
        };
        if self.provider == "wiz" {
            wiz_lights::update_saved(&app.state(), &self.id, update).await?;
        } else {
            lan_lights::update_saved(&app.state(), &self.id, update).await?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct Rule {
    light: Light,
    binding: String,
    latitude: f64,
    longitude: f64,
    location_name: String,
    armed_at_ms: i64,
    time_zone: String,
    events: Vec<i64>,
    refresh_at: i64,
    handled_at: Option<i64>,
    last_error: Option<String>,
    #[serde(default)]
    forecast_error: Option<String>,
}

impl Rule {
    fn write(&self) -> Result<(), String> {
        let value = Zeroizing::new(
            serde_json::to_string(self).map_err(|_| "Could not save the sunset schedule.")?,
        );
        if value.len() > 2500 {
            return Err("The sunset schedule could not be saved.".into());
        }
        credentials::write_labeled(&self.light.target(), "Sudo Island sunset schedule", &value)
    }

    fn read(target: &str) -> Result<Option<Self>, String> {
        let Some(value) = credentials::read(target)?.map(Zeroizing::new) else {
            return Ok(None);
        };
        let invalid = "The sunset schedule is unreadable. Disable it and set it up again.";
        let rule: Self = serde_json::from_str(&value).map_err(|_| invalid)?;
        let light =
            Light::new(rule.light.provider.clone(), rule.light.id.clone()).map_err(|_| invalid)?;
        if light.target() != target
            || sunrise_sunset::validate_coordinates(rule.latitude, rule.longitude).is_err()
        {
            return Err(invalid.into());
        }
        Ok(Some(rule))
    }

    fn refresh_with(&mut self, forecast: sunrise_sunset::Forecast, now: i64) {
        self.time_zone = forecast.tzid;
        self.events = forecast.sunsets.into_values().flatten().collect();
        self.events.sort_unstable();
        self.events.dedup();
        self.refresh_at = now + REFRESH_SECONDS;
        self.forecast_error = None;
    }

    fn due(&self, now: i64) -> Option<i64> {
        self.events.iter().copied().rev().find(|&at| {
            at.saturating_mul(1000) > self.armed_at_ms
                && at <= now
                && now.saturating_sub(at) <= CATCH_UP_SECONDS
                && self.handled_at.is_none_or(|handled| at > handled)
        })
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    enabled: bool,
    latitude: Option<f64>,
    longitude: Option<f64>,
    location_name: Option<String>,
    time_zone: Option<String>,
    next_sunset: Option<String>,
    last_error: Option<String>,
}

fn status(rule: Option<Rule>) -> Status {
    let Some(rule) = rule else {
        return Status::default();
    };
    let now = Utc::now().timestamp();
    let next_sunset = rule
        .events
        .iter()
        .copied()
        .find(|at| *at > now)
        .and_then(|at| DateTime::<Utc>::from_timestamp(at, 0))
        .map(|date| date.to_rfc3339());
    Status {
        enabled: true,
        latitude: Some(rule.latitude),
        longitude: Some(rule.longitude),
        location_name: Some(rule.location_name),
        time_zone: Some(rule.time_zone),
        next_sunset,
        last_error: rule.last_error.or(rule.forecast_error),
    }
}

impl Controller {
    async fn forecast(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<sunrise_sunset::Forecast, sunrise_sunset::Error> {
        let mut cooldown = self.provider_cooldown.lock().await;
        let now = Utc::now();
        if *cooldown > now.timestamp() {
            return Err(sunrise_sunset::Error::RateLimited {
                retry_after_secs: (*cooldown - now.timestamp()) as u64,
            });
        }
        // UTC today minus one day includes places still on the preceding civil date.
        let start = now
            .date_naive()
            .pred_opt()
            .ok_or(sunrise_sunset::Error::DateRange)?;
        let end = now
            .date_naive()
            .checked_add_days(Days::new(FORECAST_DAYS))
            .ok_or(sunrise_sunset::Error::DateRange)?;
        let result = sunrise_sunset::sunsets(latitude, longitude, start, end).await;
        if let Err(sunrise_sunset::Error::RateLimited { retry_after_secs }) = &result {
            *cooldown = Utc::now()
                .timestamp()
                .saturating_add(i64::try_from(*retry_after_secs).unwrap_or(i64::MAX));
        }
        result
    }
}

pub(crate) fn check_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "island" {
        Ok(())
    } else {
        Err("Open the schedule from Tools → Light.".into())
    }
}

#[tauri::command]
pub async fn light_sunset_status(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    provider: String,
    id: String,
) -> Result<Status, String> {
    check_window(&window)?;
    let light = Light::new(provider, id)?;
    let _guard = controller.settings.lock().await;
    Ok(status(Rule::read(&light.target())?))
}

#[tauri::command]
pub async fn light_sunset_save(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    controller: tauri::State<'_, Controller>,
    provider: String,
    id: String,
    latitude: f64,
    longitude: f64,
    location_name: Option<String>,
) -> Result<Status, String> {
    check_window(&window)?;
    let light = Light::new(provider, id)?;
    let (latitude, longitude) = sunrise_sunset::validate_coordinates(latitude, longitude)?;
    let location_name = location_name.unwrap_or_default();
    let location_name: String = location_name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(96)
        .collect();
    let binding = light.binding(&app).await?;
    let forecast = controller
        .forecast(latitude, longitude)
        .await
        .map_err(|error| error.to_string())?;
    let _guard = controller.settings.lock().await;
    if light.binding(&app).await? != binding {
        return Err("The paired light changed. Open its sunset schedule again.".into());
    }
    let now = Utc::now();
    let mut rule = Rule {
        light,
        binding,
        latitude,
        longitude,
        location_name,
        armed_at_ms: now.timestamp_millis(),
        time_zone: String::new(),
        events: Vec::new(),
        refresh_at: 0,
        handled_at: None,
        last_error: None,
        forecast_error: None,
    };
    rule.refresh_with(forecast, now.timestamp());
    rule.write()?;
    controller.changed.notify_one();
    Ok(status(Some(rule)))
}

#[tauri::command]
pub async fn light_sunset_disable(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    provider: String,
    id: String,
) -> Result<Status, String> {
    check_window(&window)?;
    let light = Light::new(provider, id)?;
    let _guard = controller.settings.lock().await;
    credentials::remove(&light.target())?;
    Ok(Status::default())
}

pub(crate) async fn forget(window: &tauri::WebviewWindow, provider: &str, id: &str) {
    if let Ok(light) = Light::new(provider.into(), id.into()) {
        let controller = window.state::<Controller>();
        let _guard = controller.settings.lock().await;
        let _ = credentials::remove(&light.target());
    }
}

pub(crate) fn init(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let controller = app.state::<Controller>();
        loop {
            if let Ok(targets) = credentials::targets(PREFIX) {
                for target in targets {
                    tick(&app, &controller, &target).await;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_INTERVAL) => {},
                _ = controller.changed.notified() => {},
            }
        }
    });
}

async fn tick(app: &tauri::AppHandle, controller: &Controller, target: &str) {
    let guard = controller.settings.lock().await;
    let Ok(Some(mut rule)) = Rule::read(target) else {
        return;
    };
    let now = Utc::now().timestamp();
    if let Some(sunset) = rule.due(now) {
        // Persist the attempt first. Reopening Sudo or manually turning the light
        // back off cannot cause a second power-on for this sunset.
        rule.handled_at = Some(sunset);
        if rule.write().is_err() {
            return;
        }
        rule.last_error = rule
            .light
            .turn_on(app, &rule.binding)
            .await
            .err()
            .map(|error| format!("Could not turn on the light at sunset. {error}"));
        let _ = rule.write();
        let _ = app.emit_to("island", "light-sunset-changed", &rule.light);
    }
    if rule.refresh_at > now {
        return;
    }
    drop(guard);
    let forecast = controller.forecast(rule.latitude, rule.longitude).await;
    let _guard = controller.settings.lock().await;
    let Ok(Some(mut current)) = Rule::read(target) else {
        return;
    };
    if current.armed_at_ms != rule.armed_at_ms {
        return;
    }
    let now = Utc::now().timestamp();
    match forecast {
        Ok(forecast) => current.refresh_with(forecast, now),
        Err(error) => {
            let delay = match &error {
                sunrise_sunset::Error::RateLimited { retry_after_secs } => {
                    i64::try_from(*retry_after_secs).unwrap_or(i64::MAX).max(1)
                }
                _ => RETRY_SECONDS,
            };
            current.refresh_at = now.saturating_add(delay);
            current.forecast_error = Some(error.to_string());
        }
    }
    if current.write().is_ok() {
        let _ = app.emit_to("island", "light-sunset-changed", &current.light);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Rule {
        Rule {
            light: Light::new("tapo".into(), "tapo".into()).unwrap(),
            binding: "fixture-id".into(),
            latitude: 36.72,
            longitude: -4.42,
            location_name: "Fixture".into(),
            armed_at_ms: 12_000_000,
            time_zone: "Europe/Madrid".into(),
            events: vec![10_000, 13_000, 99_400],
            refresh_at: i64::MAX,
            handled_at: None,
            last_error: None,
            forecast_error: None,
        }
    }

    #[test]
    fn fires_once_after_sunset_and_only_catches_up_within_the_grace_window() {
        let mut rule = example();
        assert_eq!(rule.due(12_999), None);
        assert_eq!(rule.due(13_000), Some(13_000));
        assert_eq!(rule.due(16_300), Some(13_000));
        assert_eq!(rule.due(16_601), None);
        rule.handled_at = Some(13_000);
        assert_eq!(rule.due(13_015), None);
        assert_eq!(rule.due(99_400), Some(99_400));
    }

    #[test]
    fn enabling_a_rule_after_sunset_waits_for_the_next_sunset() {
        let mut rule = example();
        rule.armed_at_ms = 13_015_500;
        assert_eq!(rule.due(13_016), None);
        assert_eq!(rule.due(99_401), Some(99_400));
    }
}
