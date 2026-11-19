//! Date-specific local sunset times from the Sunrise-Sunset v2 API.
use std::{collections::BTreeMap, fmt, time::Duration};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use reqwest::{Client, StatusCode, header::RETRY_AFTER, redirect::Policy};
use serde::{Deserialize, Deserializer};

const ENDPOINT: &str = "https://api.sunrise-sunset.org/v2";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DEFAULT_RETRY_SECONDS: u64 = 300;

#[derive(Debug)]
pub(crate) struct Forecast {
    pub tzid: String,
    pub sunsets: BTreeMap<NaiveDate, Option<i64>>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Error {
    Coordinates,
    DateRange,
    Unavailable,
    InvalidResponse,
    RateLimited { retry_after_secs: u64 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Coordinates => {
                "Enter a latitude from −90 to 90 and a longitude from −180 to 180."
            }
            Self::DateRange => "Choose a valid range of up to 366 days.",
            Self::Unavailable => "Could not reach Sunrise-Sunset. Check your internet connection.",
            Self::InvalidResponse => "Sunrise-Sunset returned invalid sunset times.",
            Self::RateLimited { .. } => {
                "Sunrise-Sunset has asked Sudo to wait before trying again."
            }
        })
    }
}

impl std::error::Error for Error {}

pub(crate) fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(f64, f64), String> {
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err(Error::Coordinates.to_string());
    }

    // A neighbourhood-scale location is sufficient to schedule sunset.
    Ok((
        (latitude * 100.0).round() / 100.0,
        (longitude * 100.0).round() / 100.0,
    ))
}

#[derive(Deserialize)]
struct ApiResponse {
    tzid: String,
    days: Vec<ApiDay>,
}

#[derive(Deserialize)]
struct ApiDay {
    date: String,
    // An explicit null is valid. An omitted sunset is a malformed record.
    #[serde(deserialize_with = "deserialize_sunset")]
    sunset: Option<i64>,
}

fn deserialize_sunset<'de, D: Deserializer<'de>>(input: D) -> Result<Option<i64>, D::Error> {
    Option::deserialize(input)
}

fn range_length(start: NaiveDate, end: NaiveDate) -> Result<usize, Error> {
    let days = (end - start).num_days() + 1;
    if !(1..=366).contains(&days) || start.year() < 1 || end.year() > 9999 {
        return Err(Error::DateRange);
    }
    Ok(days as usize)
}

fn parse_forecast(body: &[u8], start: NaiveDate, end: NaiveDate) -> Result<Forecast, Error> {
    let length = range_length(start, end)?;
    let response: ApiResponse = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    if response.days.len() != length
        || response.tzid.is_empty()
        || response.tzid.len() > 128
        || !response
            .tzid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_+-".contains(&byte))
    {
        return Err(Error::InvalidResponse);
    }
    let mut sunsets = BTreeMap::new();
    for row in response.days {
        let date =
            NaiveDate::parse_from_str(&row.date, "%Y-%m-%d").map_err(|_| Error::InvalidResponse)?;
        if !(start..=end).contains(&date) {
            return Err(Error::InvalidResponse);
        }
        if let Some(timestamp) = row.sunset {
            let utc_day = DateTime::from_timestamp(timestamp, 0)
                .ok_or(Error::InvalidResponse)?
                .date_naive();
            // A local evening can be on a neighbouring UTC date near the date line.
            if (utc_day - date).num_days().abs() > 1 {
                return Err(Error::InvalidResponse);
            }
        }
        if sunsets.insert(date, row.sunset).is_some() {
            return Err(Error::InvalidResponse);
        }
    }
    Ok(Forecast {
        tzid: response.tzid,
        sunsets,
    })
}

fn retry_seconds(value: Option<&str>, now: DateTime<Utc>) -> u64 {
    if let Some(value) = value {
        let value = value.trim();
        if let Ok(seconds) = value.parse::<u64>() {
            return seconds.max(1);
        }
        if let Ok(deadline) = DateTime::parse_from_rfc2822(value) {
            return (deadline.timestamp().saturating_sub(now.timestamp())).max(1) as u64;
        }
    }
    DEFAULT_RETRY_SECONDS
}

pub(crate) async fn sunsets(
    latitude: f64,
    longitude: f64,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Forecast, Error> {
    let (latitude, longitude) =
        validate_coordinates(latitude, longitude).map_err(|_| Error::Coordinates)?;
    range_length(start, end)?;
    let mut response = Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| Error::Unavailable)?
        .get(ENDPOINT)
        .query(&[
            ("lat", format!("{latitude:.2}")),
            ("lng", format!("{longitude:.2}")),
            ("date_start", start.format("%Y-%m-%d").to_string()),
            ("date_end", end.format("%Y-%m-%d").to_string()),
            ("time_format", "unix".to_owned()),
        ])
        .send()
        .await
        .map_err(|_| Error::Unavailable)?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok());
        return Err(Error::RateLimited {
            retry_after_secs: retry_seconds(retry_after, Utc::now()),
        });
    }
    if !response.status().is_success() {
        return Err(Error::Unavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Error::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    parse_forecast(&body, start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    #[test]
    fn accepts_unix_sunsets_and_explicitly_missing_polar_events() {
        let response = br#"{"tzid":"Arctic/Longyearbyen","days":[
            {"date":"2026-09-04","sunset":1788547305},
            {"date":"2026-09-05","sunset":null,"sun_status":"midnight_sun"}
        ]}"#;
        let forecast = parse_forecast(response, day(4), day(5)).unwrap();
        assert_eq!(forecast.tzid, "Arctic/Longyearbyen");
        assert_eq!(forecast.sunsets[&day(4)], Some(1788547305));
        assert_eq!(forecast.sunsets[&day(5)], None);
    }

    #[test]
    fn rejects_invalid_incomplete_and_duplicate_forecasts() {
        for response in [
            r#"{"tzid":"UTC","days":[{"date":"2026-09-04"}]}"#,
            r#"{"tzid":"UTC","days":[{"date":"2026-09-04","sunset":0}]}"#,
            r#"{"tzid":"","days":[{"date":"2026-09-04","sunset":1788547305}]}"#,
            r#"{"tzid":"UTC","days":[{"date":"2026-09-05","sunset":null}]}"#,
            r#"{"tzid":"UTC","days":[{"date":"2026-09-04","sunset":"1788547305"}]}"#,
            r#"{"tzid":"UTC","days":[]}"#,
        ] {
            assert_eq!(
                parse_forecast(response.as_bytes(), day(4), day(4)).unwrap_err(),
                Error::InvalidResponse
            );
        }
        let duplicate = br#"{"tzid":"UTC","days":[
            {"date":"2026-09-04","sunset":1788547305},{"date":"2026-09-04","sunset":1788547305}
        ]}"#;
        assert_eq!(
            parse_forecast(duplicate, day(4), day(5)).unwrap_err(),
            Error::InvalidResponse
        );
    }

    #[test]
    fn rounds_coordinates_and_rejects_invalid_input() {
        assert_eq!(
            validate_coordinates(36.72016, -4.425340).unwrap(),
            (36.72, -4.43)
        );
        for (lat, lng) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (90.01, 0.0),
            (0.0, -180.01),
        ] {
            assert!(validate_coordinates(lat, lng).is_err());
        }
        assert_eq!(range_length(day(5), day(4)), Err(Error::DateRange));
    }

    #[test]
    fn respects_retry_after_seconds_and_http_dates() {
        let now = DateTime::parse_from_rfc3339("2026-09-04T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(retry_seconds(Some("3600"), now), 3600);
        assert_eq!(
            retry_seconds(Some("Fri, 04 Sep 2026 12:01:00 GMT"), now),
            60
        );
        assert_eq!(retry_seconds(Some("invalid"), now), DEFAULT_RETRY_SECONDS);
    }
}
