use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::sunrise_sunset::validate_coordinates;

#[derive(Serialize)]
pub struct Place {
    latitude: f64,
    longitude: f64,
    label: String,
}

#[derive(Deserialize)]
struct CityResponse {
    #[serde(default)]
    results: Vec<City>,
}

#[derive(Deserialize)]
struct City {
    name: String,
    latitude: f64,
    longitude: f64,
    admin1: Option<String>,
    country: Option<String>,
}

#[tauri::command]
pub async fn light_sunset_search(
    window: tauri::WebviewWindow,
    query: String,
) -> Result<Vec<Place>, String> {
    crate::light_sunset::check_window(&window)?;
    let query = query.trim();
    if !(2..=75).contains(&query.chars().count()) || query.chars().any(char::is_control) {
        return Err("Enter a city or town name.".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(7))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "City search is unavailable.")?;
    let mut response = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", query),
            ("count", "6"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "City search is unavailable. You can enter coordinates manually.")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read city search results.")?
    {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err("City search returned too much data.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let found: CityResponse =
        serde_json::from_slice(&bytes).map_err(|_| "Could not read city search results.")?;
    Ok(found
        .results
        .into_iter()
        .take(6)
        .filter_map(|city| {
            let (latitude, longitude) = validate_coordinates(city.latitude, city.longitude).ok()?;
            let mut parts = Vec::new();
            for component in [Some(city.name), city.admin1, city.country]
                .into_iter()
                .flatten()
            {
                let component = crate::light_types::device_name(&component);
                if !component.is_empty() && !parts.contains(&component) {
                    parts.push(component);
                }
            }
            (!parts.is_empty()).then(|| Place {
                latitude,
                longitude,
                label: parts.join(", "),
            })
        })
        .collect())
}

#[tauri::command]
pub async fn light_sunset_detect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<Place, String> {
    crate::light_sunset::check_window(&window)?;
    detect(app).await
}

#[cfg(windows)]
async fn detect(app: tauri::AppHandle) -> Result<Place, String> {
    use windows::{
        Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy},
        Foundation::TimeSpan,
    };

    const LOCATION_ERROR: &str =
        "Windows could not find your location. Search for your city or enter coordinates.";
    const ACCESS_ERROR: &str =
        "Allow location in Windows Settings → Privacy & security → Location, or enter your city.";
    crate::focus::island_focus(app.clone(), true).map_err(|_| ACCESS_ERROR)?;
    let result = async {
        let (send, receive) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = send.send(Geolocator::RequestAccessAsync());
        })
        .map_err(|_| ACCESS_ERROR)?;
        let access = receive
            .await
            .map_err(|_| ACCESS_ERROR)?
            .map_err(|_| ACCESS_ERROR)?;
        let status =
            tokio::time::timeout(Duration::from_secs(45), access.clone().into_future()).await;
        let status = match status {
            Ok(result) => result.map_err(|_| ACCESS_ERROR)?,
            Err(_) => {
                let _ = access.Cancel();
                return Err(ACCESS_ERROR.into());
            }
        };
        if status != GeolocationAccessStatus::Allowed {
            return Err(ACCESS_ERROR.into());
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let fix = Geolocator::new().and_then(|locator| {
                locator.SetDesiredAccuracy(PositionAccuracy::Default)?;
                locator.GetGeopositionAsyncWithAgeAndTimeout(
                    TimeSpan {
                        Duration: 3600 * 10_000_000,
                    },
                    TimeSpan {
                        Duration: 12 * 10_000_000,
                    },
                )
            });
            let _ = send.send(fix);
        })
        .map_err(|_| LOCATION_ERROR)?;
        let fix = receive
            .await
            .map_err(|_| LOCATION_ERROR)?
            .map_err(|_| LOCATION_ERROR)?;
        let point = tokio::time::timeout(Duration::from_secs(15), fix.clone().into_future()).await;
        let point = match point {
            Ok(result) => result
                .and_then(|fix| fix.Coordinate())
                .and_then(|coordinate| coordinate.Point())
                .and_then(|point| point.Position())
                .map_err(|_| LOCATION_ERROR)?,
            Err(_) => {
                let _ = fix.Cancel();
                return Err(LOCATION_ERROR.into());
            }
        };
        let (latitude, longitude) = validate_coordinates(point.Latitude, point.Longitude)?;
        Ok(Place {
            latitude,
            longitude,
            label: "Windows location".into(),
        })
    }
    .await;
    let _ = crate::focus::island_focus(app, false);
    result
}

#[cfg(not(windows))]
async fn detect(_: tauri::AppHandle) -> Result<Place, String> {
    Err(
        "Location detection is available on Windows. Search for your city or enter coordinates."
            .into(),
    )
}
