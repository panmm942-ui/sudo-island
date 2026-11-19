//! Read and control the Spotify and Apple Music Windows media sessions.

#[path = "spotify_actions.rs"]
pub(crate) mod actions;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provider {
    #[default]
    Spotify,
    AppleMusic,
}

const APPLE_MUSIC_KEY_PREFIX: &str = "appleMusic:";
const APPLE_MUSIC_PACKAGE: &str = "appleinc.applemusicwin_nzyj5cx40ttqa";

fn provider_for_source(source: &str) -> Option<Provider> {
    let source = source.to_ascii_lowercase();
    let executable = source.rsplit(['\\', '/']).next();
    if source == "spotify"
        || executable == Some("spotify.exe")
        || source.starts_with("spotifyab.spotifymusic_")
        || source == "com.spotify.client"
    {
        return Some(Provider::Spotify);
    }
    if source == APPLE_MUSIC_PACKAGE
        || source.strip_prefix(APPLE_MUSIC_PACKAGE).is_some_and(|suffix| {
            suffix.strip_prefix('!').is_some_and(|app_id| !app_id.is_empty())
        })
        || executable == Some("applemusic.exe")
    {
        return Some(Provider::AppleMusic);
    }
    None
}

fn track_key(provider: Provider, title: &str, artist: &str, album: &str) -> String {
    let mut key = serde_json::json!([title, artist, album]).to_string();
    if provider == Provider::AppleMusic {
        key.insert_str(0, APPLE_MUSIC_KEY_PREFIX);
    }
    key
}

fn provider_for_key(key: Option<&str>) -> Provider {
    if key.is_some_and(|value| value.starts_with(APPLE_MUSIC_KEY_PREFIX)) {
        Provider::AppleMusic
    } else {
        Provider::Spotify
    }
}

fn session_priority(provider: Provider, playing: bool, ready: bool, current: bool) -> (bool, bool, bool, bool) {
    (playing, ready, current, provider == Provider::Spotify)
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    provider: Provider,
    bridge_revision: Option<String>,
    canvas_revision: Option<String>,
    key: String,
    title: String,
    artist: String,
    album: String,
    duration: Option<f64>,
    position: Option<f64>,
    position_updated_at: u64,
    artwork: Option<String>,
    playing: bool,
    can_toggle: bool,
    can_previous: bool,
    can_next: bool,
    can_seek: bool,
    liked: Option<bool>,
    like_error: Option<String>,
    can_like: bool,
    shuffle: Option<bool>,
    repeat_mode: Option<String>,
    shuffle_error: Option<String>,
    repeat_error: Option<String>,
    can_shuffle: bool,
    can_repeat: bool,
    session_error: Option<String>,
}

/// Used by an explicit in-island selection immediately before its API write.
/// This only inspects Windows media state; it never activates the Spotify UI.
pub(crate) fn current_native_track(expected_key:&str)->Result<Track,String> {
    #[cfg(windows)]
    {
        let track=platform::state(Some(expected_key.to_owned()), Some(Provider::Spotify)).map_err(|_|"Could not verify the current Windows Spotify song.")?
            .ok_or("Spotify has no active Windows session. Start playback in Spotify before choosing a song.")?;
        if track.key!=expected_key{return Err("Spotify changed songs. Choose the search result again.".into());}
        Ok(track)
    }
    #[cfg(not(windows))]
    {let _=expected_key;Err("Spotify playback is available in the Windows app.".into())}
}
/// Opens exactly one song in the installed Spotify app as `spotify:track:<id>`.
/// Only a strictly validated 22-character base62 track id is accepted; this is
/// the single narrow URI launch used by in-island search when Spotify's Web API
/// can't start playback (rate limited, not connected or no usable device).
/// The key of the song Windows reports for Spotify right now (None without a Spotify session).
pub(crate) fn current_key()->Option<String>{
    #[cfg(windows)]
    {platform::state(None, Some(Provider::Spotify)).ok().flatten().filter(|track| !track.title.is_empty()).map(|track|track.key)}
    #[cfg(not(windows))]
    {None}
}

pub(crate) fn open_track_in_app(id:&str)->Result<(),String> {
    if id.len()!=22||!id.bytes().all(|byte|byte.is_ascii_alphanumeric()){return Err("This Spotify song link is invalid. Search again.".into());}
    #[cfg(windows)]
    {
        match platform::launch_uri(&format!("spotify:track:{id}")) {
            Ok(true)=>Ok(()),
            _=>Err("Windows could not open this song in the Spotify app. Make sure Spotify is installed.".into()),
        }
    }
    #[cfg(not(windows))]
    {Err("Opening Spotify is available on Windows.".into())}
}
/// Opens the installed Spotify app's own search for one percent-encoded query
/// (`spotify:search:...`, the same launch as the `spotify_search` command). Used
/// only when a Deezer fallback result can't be matched to a Spotify track id.
pub(crate) fn open_search_in_app(query:&str)->Result<(),String> {
    let uri=search_uri(query)?;
    #[cfg(windows)]
    {
        match platform::launch_uri(&uri) {
            Ok(true)=>Ok(()),
            _=>Err("Windows could not open Spotify's search. Make sure Spotify is installed.".into()),
        }
    }
    #[cfg(not(windows))]
    {let _=uri;Err("Opening Spotify is available on Windows.".into())}
}
/// Validate an incoming client packet against Windows without decoding a thumbnail.
pub(crate) fn current_bridge_metadata()->Result<Track,String>{
    #[cfg(windows)]{platform::metadata().map_err(|_|"Could not verify Windows Spotify metadata.".to_string())?.ok_or_else(||"No current Windows Spotify session.".into())}
    #[cfg(not(windows))]{Err("Windows Spotify metadata is unavailable.".into())}
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Previous,
    Toggle,
    Next,
    Pause,
    Play,
    Like,
    Shuffle,
    Repeat,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct Features { like: bool, shuffle:bool, repeat:bool }

fn search_uri(query:&str) -> Result<String,String> {
    let query=query.trim();
    if query.is_empty() || query.chars().count()>200 { return Err("Enter a song or artist, up to 200 characters.".into()); }
    let mut encoded=String::new();
    for byte in query.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) { encoded.push(*byte as char); }
        else { use std::fmt::Write; let _=write!(encoded,"%{byte:02X}"); }
    }
    Ok(format!("spotify:search:{encoded}"))
}

#[tauri::command]
pub async fn spotify_search(query:String) -> Result<bool,String> {
    let uri=search_uri(&query)?;
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move|| {
        platform::launch_uri(&uri).map_err(|e|e.to_string())
    }).await.map_err(|e|e.to_string())?;
    #[cfg(not(windows))]
    {let _=uri;Err("Spotify search is available on Windows.".into())}
}

/// Open the installed Spotify client without selecting a song or changing playback.
#[tauri::command]
pub async fn spotify_open() -> Result<bool, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(|| platform::launch_uri("spotify:").map_err(|error| error.to_string()))
        .await.map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    Err("Opening Spotify is available on Windows.".into())
}

#[cfg(test)]
mod search_tests {
    #[test]
    fn search_encodes_text_as_data_and_rejects_empty_input() {
        assert_eq!(super::search_uri(" High Class & Wang ").unwrap(),"spotify:search:High%20Class%20%26%20Wang");
        assert!(super::search_uri("Καλησπέρα").unwrap().contains("%CE"));
        assert!(super::search_uri(" ").is_err());
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn recognizes_supported_desktop_media_sessions() {
        assert_eq!(provider_for_source("AppleInc.AppleMusicWin_nzyj5cx40ttqa!App"), Some(Provider::AppleMusic));
        assert_eq!(provider_for_source("appleinc.applemusicwin_nzyj5cx40ttqa"), Some(Provider::AppleMusic));
        assert_eq!(provider_for_source("AppleMusic.exe"), Some(Provider::AppleMusic));
        assert_eq!(provider_for_source("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), Some(Provider::Spotify));
        assert_eq!(provider_for_source(r"C:\Program Files\Spotify\Spotify.exe"), Some(Provider::Spotify));
        assert_eq!(provider_for_source("AppleInc.AppleMusicWin_nzyj5cx40ttqaUnexpected!App"), None);
        assert_eq!(provider_for_source("AppleInc.iTunes_nzyj5cx40ttqa!App"), None);
        assert_eq!(provider_for_source("MSEdge"), None);
    }

    #[test]
    fn routes_the_same_song_to_its_provider_and_preserves_spotify_keys() {
        let spotify = track_key(Provider::Spotify, "Song", "Artist", "Album");
        let apple = track_key(Provider::AppleMusic, "Song", "Artist", "Album");
        assert_eq!(spotify, r#"["Song","Artist","Album"]"#);
        assert_ne!(apple, spotify);
        assert_eq!(provider_for_key(Some(&spotify)), Provider::Spotify);
        assert_eq!(provider_for_key(Some(&apple)), Provider::AppleMusic);
        assert_eq!(provider_for_key(None), Provider::Spotify);
        let legacy: Track = serde_json::from_value(serde_json::json!({"title": "Song"})).unwrap();
        assert_eq!(legacy.provider, Provider::Spotify);
        assert_eq!(serde_json::to_value(Track { provider: Provider::AppleMusic, ..Default::default() }).unwrap()["provider"], "appleMusic");
    }

    #[test]
    fn prefers_playing_music_and_uses_windows_current_session_to_break_ties() {
        assert!(session_priority(Provider::AppleMusic, true, true, false) > session_priority(Provider::Spotify, false, true, true));
        assert!(session_priority(Provider::AppleMusic, true, true, true) > session_priority(Provider::Spotify, true, true, false));
        assert!(session_priority(Provider::Spotify, false, true, false) > session_priority(Provider::AppleMusic, false, false, true));
        assert!(session_priority(Provider::Spotify, false, true, false) > session_priority(Provider::AppleMusic, false, true, false));
    }
}

#[cfg(windows)]
mod platform {
    use super::{Action, Provider, Track, provider_for_key, provider_for_source, session_priority, track_key};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::DataReader;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows::core::Result;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

    // Tokio's blocking workers are reused. Balance initialization on the worker
    // that owns each WinRT call rather than relying on another feature's COM setup.
    struct Apartment;
    impl Apartment {
        fn new() -> Result<Self> {
            unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;
            Ok(Self)
        }
    }
    impl Drop for Apartment { fn drop(&mut self) { unsafe { RoUninitialize() }; } }

    fn context(stage: &str, error: windows::core::Error) -> windows::core::Error {
        windows::core::Error::new(error.code(), format!("Windows music {stage} failed ({:#010X}).", error.code().0 as u32))
    }

    pub fn launch_uri(value: &str) -> Result<bool> {
        let _apartment = Apartment::new()?;
        let uri = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(value))?;
        windows::System::Launcher::LaunchUriAsync(&uri)?.join()
    }

    // Log transitions only, with no song, account, token or artwork data.
    fn report_state(value: &str) {
        static LAST: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
        if let Ok(mut last) = LAST.get_or_init(|| std::sync::Mutex::new(String::new())).lock() {
            if last.as_str() != value {
                eprintln!("Music state: {value}");
                *last = value.to_owned();
            }
        }
    }

    fn session(required: Option<Provider>) -> Result<Option<(Provider, Session)>> {
        let manager = Manager::RequestAsync().and_then(|operation| operation.join()).map_err(|error| context("media session request", error))?;
        let sessions = manager.GetSessions().map_err(|error| context("media session enumeration", error))?;
        let current = manager.GetCurrentSession().ok();
        let mut selected = None;
        let mut best = None;
        for index in 0..sessions.Size()? {
            let Ok(session) = sessions.GetAt(index) else { continue; };
            let Ok(source) = session.SourceAppUserModelId() else { continue; };
            let Some(provider) = provider_for_source(&source.to_string()) else { continue; };
            if required.is_some_and(|wanted| provider != wanted) { continue; }
            let status = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).ok();
            let priority = session_priority(
                provider,
                status == Some(Status::Playing),
                matches!(status, Some(Status::Playing | Status::Paused)),
                current.as_ref() == Some(&session),
            );
            if best.is_none_or(|old| priority > old) {
                best = Some(priority);
                selected = Some((provider, session));
            }
        }
        Ok(selected)
    }

    fn running_app(required: Option<Provider>) -> Option<Provider> {
        // A client may be open before it publishes any media session.
        let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
            return None;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut result = unsafe { Process32FirstW(snapshot, &mut entry) };
        let mut found = None;
        while result.is_ok() {
            let end = entry.szExeFile.iter().position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let executable = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if let Some(provider) = provider_for_source(&executable)
                && required.is_none_or(|wanted| provider == wanted)
            {
                found = Some(provider);
                if required.is_some() || provider == Provider::Spotify { break; }
            }
            result = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        let _ = unsafe { CloseHandle(snapshot) };
        found
    }

    fn artwork(properties: &Properties) -> Result<Option<String>> {
        let stream = properties.Thumbnail()?.OpenReadAsync()?.join()?;
        let size = stream.Size()?;
        if size == 0 || size > 2 * 1024 * 1024 {
            return Ok(None);
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        let loaded = reader.LoadAsync(size as u32)?.join()?;
        let mut bytes = vec![0; loaded as usize];
        reader.ReadBytes(&mut bytes)?;
        let mime = match bytes.as_slice() {
            [0xff, 0xd8, 0xff, ..] => "image/jpeg",
            [137, 80, 78, 71, 13, 10, 26, 10, ..] => "image/png",
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
            _ => return Ok(None),
        };
        Ok(Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes))))
    }

    pub fn state(known_track: Option<String>, required: Option<Provider>) -> Result<Option<Track>> {
        let result = Apartment::new().map_err(|error| context("Windows Runtime initialization", error))
            .and_then(|_apartment| {
                match session(required)? {
                    Some((provider, session)) => read_session(&session, provider, known_track.as_deref(), true).map(Some),
                    None => Ok(running_app(required).map(|provider| Track { provider, ..Default::default() })),
                }
            });
        match result {
            Ok(track) => {
                report_state(match track.as_ref() { Some(track) if !track.title.is_empty() => "active session", Some(_) => "client open; waiting for media", None => "client not running" });
                Ok(track)
            }
            Err(error) => {
                let message = error.to_string();
                report_state(&message);
                if let Some(provider) = running_app(required) {
                    Ok(Some(Track { provider, session_error: Some(message), ..Default::default() }))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub fn metadata() -> Result<Option<Track>> {
        let _apartment = Apartment::new()?;
        session(Some(Provider::Spotify))?.map(|(provider, session)| read_session(&session, provider, None, false)).transpose()
    }

    fn read_session(session: &Session, provider: Provider, known_track: Option<&str>, include_artwork: bool) -> Result<Track> {
        let properties = session.TryGetMediaPropertiesAsync().and_then(|operation| operation.join()).map_err(|error| context("media properties read", error))?;
        let title = properties.Title()?.to_string();
        let artist = properties.Artist()?.to_string();
        let album = properties.AlbumTitle()?.to_string();
        let timeline = session.GetTimelineProperties().ok();
        let duration = timeline.as_ref().and_then(|timeline| {
            let seconds = (timeline.EndTime().ok()?.Duration - timeline.StartTime().ok()?.Duration) as f64 / 10_000_000.0;
            (1.0..=3600.0).contains(&seconds).then_some(seconds.round())
        });
        let key = track_key(provider, &title, &artist, &album);
        let info = session.GetPlaybackInfo()?;
        let playing = info.PlaybackStatus()? == Status::Playing;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let position = timeline.as_ref().and_then(|timeline| {
            let base = (timeline.Position().ok()?.Duration - timeline.StartTime().ok()?.Duration) as f64 / 10_000_000.0;
            let updated = timeline.LastUpdatedTime().ok()?.UniversalTime;
            let updated_ms = (updated as f64 - 116_444_736_000_000_000.0) / 10_000.0;
            let elapsed = if playing && updated_ms > 0.0 {
                ((now.as_millis() as f64 - updated_ms) / 1000.0).clamp(0.0, 3600.0)
            } else { 0.0 };
            Some((base + elapsed).max(0.0).min(duration.unwrap_or(3600.0)))
        });
        let controls = info.Controls()?;
        let (shuffle, repeat_mode) = if provider == Provider::Spotify {
            let reported_shuffle = info.IsShuffleActive().and_then(|value| value.Value()).ok();
            let reported_repeat = info.AutoRepeatMode().and_then(|value| value.Value()).ok().and_then(|mode| {
                use windows::Media::MediaPlaybackAutoRepeatMode as Repeat;
                if mode == Repeat::None { Some("off") } else if mode == Repeat::List { Some("context") } else if mode == Repeat::Track { Some("track") } else { None }
            }).map(str::to_owned);
            super::options::effective(reported_shuffle, reported_repeat)
        } else {
            (None, None)
        };
        let can_shuffle = provider == Provider::Spotify && controls.IsShuffleEnabled().unwrap_or(false);
        let can_repeat = provider == Provider::Spotify && controls.IsRepeatEnabled().unwrap_or(false);
        let artwork = if !include_artwork || known_track == Some(key.as_str()) || title.is_empty() {
            None
        } else {
            artwork(&properties).ok().flatten()
        };
        Ok(Track {
            provider,
            key,
            title,
            artist,
            album,
            duration,
            position,
            position_updated_at: now.as_millis() as u64,
            artwork,
            playing,
            can_toggle: controls.IsPlayPauseToggleEnabled()?
                || controls.IsPlayEnabled()?
                || controls.IsPauseEnabled()?,
            can_previous: controls.IsPreviousEnabled()?,
            can_next: controls.IsNextEnabled()?,
            can_seek: controls.IsPlaybackPositionEnabled().unwrap_or(false),
            shuffle_error: (provider == Provider::Spotify && !can_shuffle).then(|| "Spotify does not offer shuffle to Windows right now.".into()),
            repeat_error: (provider == Provider::Spotify && !can_repeat).then(|| "Spotify does not offer repeat to Windows right now.".into()),
            can_shuffle,
            can_repeat,
            shuffle,
            repeat_mode,
            ..Default::default()
        })
    }

    fn matches_track(session: &Session, provider: Provider, expected: Option<&str>) -> Result<bool> {
        let Some(expected) = expected else { return Ok(true); };
        let properties = session.TryGetMediaPropertiesAsync()?.join()?;
        let actual = track_key(provider, &properties.Title()?.to_string(), &properties.Artist()?.to_string(), &properties.AlbumTitle()?.to_string());
        Ok(actual == expected)
    }

    /// Jump to `seconds` into the current song, only if it is still the expected one.
    pub fn seek(seconds: f64, key: Option<String>) -> Result<bool> {
        let _apartment = Apartment::new()?;
        let Some((provider, session)) = session(Some(provider_for_key(key.as_deref())))? else { return Ok(false); };
        if !matches_track(&session, provider, key.as_deref())? { return Ok(false); }
        if !session.GetPlaybackInfo()?.Controls()?.IsPlaybackPositionEnabled()? { return Ok(false); }
        let timeline = session.GetTimelineProperties()?;
        let start = timeline.StartTime()?.Duration;
        let end = timeline.EndTime()?.Duration;
        let target = (start + (seconds.max(0.0) * 10_000_000.0) as i64).min(end.max(start));
        session.TryChangePlaybackPositionAsync(target)?.join()
    }

    pub fn control(action: Action, key: Option<String>) -> Result<bool> {
        let _apartment = Apartment::new()?;
        let Some((provider, session)) = session(Some(provider_for_key(key.as_deref())))? else {
            return Ok(false);
        };
        if !matches_track(&session, provider, key.as_deref())? { return Ok(false); }
        match action {
            Action::Like => Ok(false), // Web API action is handled by spotify_control.
            Action::Shuffle | Action::Repeat => {
                if provider != Provider::Spotify { return Ok(false); }
                // Spotify obeys Windows' shuffle/repeat commands but never reports the new state
                // (verified: the change happens in Spotify, Windows keeps the old flags). So the
                // command goes through Windows only and Sudo remembers what it set. No web call:
                // the old web fallback polled Spotify every ~2 s and got the connection blocked.
                let track = read_session(&session, provider, key.as_deref(), false)?;
                if key.as_ref().is_some_and(|expected| expected != &track.key) { return Ok(false); }
                let info = session.GetPlaybackInfo()?;
                let reported_shuffle = info.IsShuffleActive().and_then(|value| value.Value()).ok();
                let reported_repeat = info.AutoRepeatMode().and_then(|value| value.Value()).ok();
                use windows::Media::MediaPlaybackAutoRepeatMode as Repeat;
                if matches!(action, Action::Shuffle) {
                    if !info.Controls()?.IsShuffleEnabled()? { return Ok(false); }
                    let requested = !track.shuffle.unwrap_or(false);
                    if !session.TryChangeShuffleActiveAsync(requested)?.join()? { return Ok(false); }
                    super::options::remember_shuffle(requested, reported_shuffle);
                } else {
                    if !info.Controls()?.IsRepeatEnabled()? { return Ok(false); }
                    let next = super::actions::next_repeat(track.repeat_mode.as_deref().unwrap_or("off")).unwrap_or("context");
                    let requested = match next { "context" => Repeat::List, "track" => Repeat::Track, _ => Repeat::None };
                    if !session.TryChangeAutoRepeatModeAsync(requested)?.join()? { return Ok(false); }
                    super::options::remember_repeat(next, reported_repeat.map(|mode| mode.0));
                }
                Ok(true)
            }
            Action::Previous => session.TrySkipPreviousAsync()?.join(),
            Action::Next => session.TrySkipNextAsync()?.join(),
            Action::Pause => {
                if session.GetPlaybackInfo()?.PlaybackStatus()? != Status::Playing { return Ok(false); }
                session.TryPauseAsync()?.join()
            }
            Action::Play => {
                if session.GetPlaybackInfo()?.PlaybackStatus()? != Status::Paused { return Ok(false); }
                session.TryPlayAsync()?.join()
            }
            Action::Toggle => {
                let info = session.GetPlaybackInfo()?;
                if info.Controls()?.IsPlayPauseToggleEnabled()? {
                    session.TryTogglePlayPauseAsync()?.join()
                } else if info.PlaybackStatus()? == Status::Playing {
                    session.TryPauseAsync()?.join()
                } else {
                    session.TryPlayAsync()?.join()
                }
            }
        }
    }
}

#[tauri::command]
pub async fn spotify_state(known_track: Option<String>, features: Option<Features>) -> Result<Option<Track>, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || {
        let mut track = platform::state(known_track, None).map_err(|error| error.to_string())?;
        let features=features.unwrap_or_default();
        if features.like {
            if let Some(value) = track.as_mut().filter(|track| track.provider == Provider::Spotify && !track.title.is_empty()) {
                actions::decorate(value);
                // A network lookup may outlive the song. Never attach its result to the next one.
                let fresh = platform::state(Some(value.key.clone()), None).map_err(|error| error.to_string())?;
                if fresh.as_ref().map(|track| &track.key) != Some(&value.key) { return Ok(fresh); }
            }
        }
        if let Some(value)=track.as_mut().filter(|track| track.provider == Provider::Spotify){
            if let Some(local)=crate::spotify_bridge::matching_snapshot(&value.key,&value.title,&value.artist,Some(&value.album),value.duration){
                value.bridge_revision=Some(serde_json::json!([local.uri,local.images.iter().map(|image|&image.url).collect::<Vec<_>>(),local.canvas_url]).to_string());
                value.canvas_revision=Some(serde_json::json!([local.uri,local.canvas_url,local.canvas_error]).to_string());
                value.position=Some(local.position);value.position_updated_at=local.position_updated_at;
                value.playing=local.playing;
            }
        }
        Ok(track)
    })
        .await
        .map_err(|err| err.to_string())?
        ;
    #[cfg(not(windows))]
    {
        let _ = (known_track, features);
        Ok(None)
    }
}

#[tauri::command]
pub async fn spotify_control(action: Action, track_key: Option<String>) -> Result<bool, String> {
    if matches!(action, Action::Like | Action::Shuffle | Action::Repeat) && track_key.as_ref().is_none_or(|key| key.is_empty()) {
        return Err("The current Spotify song is required. Try again.".into());
    }
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || {
        if matches!(action, Action::Like) {
            let expected = track_key.as_deref().unwrap_or_default();
            if provider_for_key(Some(expected)) != Provider::Spotify { return Ok(false); }
            let track = platform::state(Some(expected.to_owned()), Some(Provider::Spotify)).map_err(|error| error.to_string())?.filter(|track| track.key == expected).ok_or("Spotify changed songs. Try again.")?;
            return actions::toggle(&track, || {
                let actual = platform::state(Some(expected.to_owned()), Some(Provider::Spotify)).map_err(|error| error.to_string())?;
                if actual.as_ref().is_some_and(|track| track.key == expected) { Ok(()) } else { Err("Spotify changed songs. Try again.".into()) }
            });
        }
        platform::control(action, track_key).map_err(|error| error.to_string())
    })
        .await
        .map_err(|err| err.to_string())?
        ;
    #[cfg(not(windows))]
    {
        let _ = (action, track_key);
        Ok(false)
    }
}

#[tauri::command]
pub async fn spotify_seek(seconds: f64, track_key: Option<String>) -> Result<bool, String> {
    if !seconds.is_finite() || !(0.0..=86_400.0).contains(&seconds) { return Err("Invalid song position.".into()); }
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || platform::seek(seconds, track_key).map_err(|error| error.to_string()))
        .await
        .map_err(|err| err.to_string())?;
    #[cfg(not(windows))]
    {
        let _ = (seconds, track_key);
        Ok(false)
    }
}

#[cfg(all(test, windows))]
mod timeline_tests {
    #[test]
    #[ignore = "explicit read-only current Windows thumbnail pixel-size probe; no API requests"]
    fn spotify_thumbnail_dimensions_probe() {
        use base64::Engine;
        let track=super::platform::state(None, Some(super::Provider::Spotify)).expect("Windows Spotify state").expect("active Spotify session");
        let art=track.artwork.expect("Windows Spotify thumbnail");
        let bytes=base64::engine::general_purpose::STANDARD.decode(art.split_once(',').expect("image data URI").1).expect("image bytes");
        let path=std::env::temp_dir().join("sudo-current-smtc-cover-probe");
        std::fs::write(&path,&bytes).expect("write temporary image for pixel inspection");
        println!("SMTC_COVER_PROBE path={} bytes={} title={} artist={} album={}",path.display(),bytes.len(),track.title,track.artist,track.album);
    }
    #[test]
    #[ignore = "reads live Spotify through the production async command"]
    fn spotify_command_smoke() {
        let track = tauri::async_runtime::block_on(super::spotify_state(None, None))
            .expect("Spotify async state command failed").expect("Spotify missing");
        assert!(track.session_error.is_none(), "Spotify discovery reported an error");
        println!("async command: has_title={}, duration={:?}, playing={}", !track.title.is_empty(), track.duration, track.playing);
    }

    #[test]
    #[ignore = "reads live Spotify timeline"]
    fn spotify_timeline_smoke() {
        let track = super::platform::state(None, Some(super::Provider::Spotify)).expect("Spotify state query failed").expect("Spotify missing");
        println!("duration={:?}, position={:?}, playing={}, updated={}",track.duration,track.position,track.playing,track.position_updated_at);
        println!("shuffle={:?}, repeat={:?}, can_shuffle={}, can_repeat={}", track.shuffle, track.repeat_mode, track.can_shuffle, track.can_repeat);
        if let (Some(position), Some(duration)) = (track.position,track.duration) { assert!(position>=0.0&&position<=duration); }
    }
}

/// Shuffle/repeat as Sudo last set them. Spotify obeys Windows' commands but keeps reporting the
/// flags it had before, so a remembered choice wins until Spotify reports something different
/// (for example after the user changes it in Spotify itself and Spotify restarts its session).
pub(crate) mod options {
    use std::sync::Mutex;
    #[derive(Default)]
    struct Remembered { shuffle: Option<(bool, Option<bool>)>, repeat: Option<(String, Option<i32>)> }
    static STATE: Mutex<Remembered> = Mutex::new(Remembered { shuffle: None, repeat: None });

    fn repeat_code(mode: Option<&str>) -> Option<i32> {
        match mode { Some("off") => Some(0), Some("track") => Some(1), Some("context") => Some(2), _ => None }
    }
    pub fn remember_shuffle(value: bool, reported: Option<bool>) {
        if let Ok(mut state) = STATE.lock() { state.shuffle = Some((value, reported)); }
    }
    pub fn remember_repeat(value: &str, reported: Option<i32>) {
        if let Ok(mut state) = STATE.lock() { state.repeat = Some((value.to_owned(), reported)); }
    }
    /// What to show: the remembered value while Spotify still reports what it reported when we set it.
    pub fn effective(reported_shuffle: Option<bool>, reported_repeat: Option<String>) -> (Option<bool>, Option<String>) {
        let Ok(mut state) = STATE.lock() else { return (reported_shuffle, reported_repeat) };
        let shuffle = match state.shuffle {
            Some((value, at)) if at == reported_shuffle => Some(value),
            _ => { state.shuffle = None; reported_shuffle }
        };
        let repeat_reported = repeat_code(reported_repeat.as_deref());
        let repeat = match &state.repeat {
            Some((value, at)) if *at == repeat_reported => Some(value.clone()),
            _ => { state.repeat = None; reported_repeat }
        };
        (shuffle, repeat)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn remembered_options_win_until_spotify_reports_something_new() {
            remember_shuffle(false, Some(true));
            remember_repeat("track", Some(0));
            assert_eq!(effective(Some(true), Some("off".into())), (Some(false), Some("track".into())));
            // Spotify finally reports a different value: trust it again.
            assert_eq!(effective(Some(false), Some("context".into())), (Some(false), Some("context".into())));
            assert_eq!(effective(Some(true), Some("off".into())), (Some(true), Some("off".into())));
        }
    }
}
