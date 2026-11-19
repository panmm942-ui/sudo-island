//! Process lifetime and removal entry points. The uninstall action only opens Windows Settings.

#[cfg(windows)]
pub struct InstanceGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl InstanceGuard {
    fn acquire_named(name: &str) -> Result<Option<Self>, String> {
        use windows::{core::PCWSTR, Win32::{Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS}, System::Threading::CreateMutexW}};
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // Creating the same named object is atomic across processes. Keep its handle alive
        // until the event loop returns; closing the last handle also handles crashes cleanly.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(|error| format!("Could not check whether Sudo is already running: {error}"))?;
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Marks a copy started to replace one whose graphics were lost. It carries how many such
/// relaunches led to it (`--recover-graphics=2`) and starts before the old copy has exited.
const RECOVERY_FLAG: &str = "--recover-graphics";
/// A relaunch within this long of starting is refused, so a failure that persists cannot loop.
const RECOVERY_MIN_UPTIME: Duration = Duration::from_secs(45);
/// Relaunches in a row before Sudo stops trying. A copy that ran this long starts the count again.
const RECOVERY_LIMIT: u32 = 3;
const RECOVERY_SETTLED: Duration = Duration::from_secs(10 * 60);
const WEBVIEW_POLL: Duration = Duration::from_secs(3);
static STARTED: OnceLock<Instant> = OnceLock::new();
static EXITING: AtomicBool = AtomicBool::new(false);

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn recovery_count(mut arguments: impl Iterator<Item = String>) -> u32 {
    arguments
        .find_map(|argument| {
            let rest = argument.strip_prefix(RECOVERY_FLAG)?;
            Some(rest.strip_prefix('=').and_then(|count| count.parse().ok()).unwrap_or(1))
        })
        .unwrap_or(0)
}

#[cfg(windows)]
pub fn acquire_instance() -> Result<Option<InstanceGuard>, String> {
    STARTED.get_or_init(Instant::now);
    // One Sudo per interactive Windows session, shared by installed and portable copies.
    let name = r"Local\dev.sudo.island";
    if recovery_count(std::env::args()) == 0 {
        return InstanceGuard::acquire_named(name);
    }
    // The copy being replaced still holds the name while it shuts down.
    for _ in 0..50 {
        if let Some(guard) = InstanceGuard::acquire_named(name)? {
            return Ok(Some(guard));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(None)
}

/// The count the next copy carries, or None when this copy must not relaunch.
fn next_recovery(uptime: Duration, count: u32) -> Option<u32> {
    if uptime < RECOVERY_MIN_UPTIME {
        return None;
    }
    let count = if uptime >= RECOVERY_SETTLED { 0 } else { count };
    (count < RECOVERY_LIMIT).then_some(count + 1)
}

/// Called as the app shuts down, so losing the WebView then is not mistaken for a crash.
pub fn exiting() {
    EXITING.store(true, Ordering::Relaxed);
}

fn relaunch(app: &tauri::AppHandle) -> Result<(), String> {
    if EXITING.load(Ordering::Relaxed) {
        return Ok(());
    }
    let uptime = STARTED.get().map_or(Duration::ZERO, Instant::elapsed);
    let next = next_recovery(uptime, recovery_count(std::env::args()))
        .ok_or("Sudo restarted moments ago. Not restarting again yet.")?;
    let program = std::env::current_exe().map_err(|_| "Can't find Sudo's program file.".to_string())?;
    std::process::Command::new(program)
        .arg(format!("{RECOVERY_FLAG}={next}"))
        .spawn()
        .map_err(|_| "Could not restart Sudo.".to_string())?;
    exiting();
    app.exit(0);
    Ok(())
}

/// Relaunches Sudo after its graphics were lost for good (a display driver or compositor crash),
/// which a page cannot repair from inside the WebView.
#[tauri::command]
pub fn recover_graphics(window: tauri::WebviewWindow, app: tauri::AppHandle) -> Result<(), String> {
    if window.label() != "island" {
        return Err("Only the island can restart Sudo.".into());
    }
    relaunch(&app)
}

/// Whether this process still has its WebView2 browser process. None when Windows cannot say.
#[cfg(windows)]
fn webview_alive() -> Option<bool> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let own = std::process::id();
    // SAFETY: the snapshot handle is closed below; `entry` is a correctly sized out-parameter.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut alive = false;
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next.is_ok() {
            let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if entry.th32ParentProcessID == own
                && String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("msedgewebview2.exe")
            {
                alive = true;
                break;
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        Some(alive)
    }
}

/// After repeated graphics-process crashes WebView2 shuts its browser process down, and the
/// island is gone while Sudo keeps running. Nothing inside the page can report that, so watch
/// for it here and relaunch.
pub fn watch_webview(app: &tauri::AppHandle) {
    #[cfg(windows)]
    {
        let app = app.clone();
        let _ = std::thread::Builder::new().name("webview-watch".into()).spawn(move || {
            let (mut seen, mut missing) = (false, 0);
            loop {
                std::thread::sleep(WEBVIEW_POLL);
                match webview_alive() {
                    Some(true) => (seen, missing) = (true, 0),
                    Some(false) if seen => missing += 1,
                    _ => {}
                }
                // Two misses in a row: one could be a snapshot taken mid-restart.
                if missing >= 2 {
                    let _ = relaunch(&app);
                    return;
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(not(windows))]
pub struct InstanceGuard;

#[cfg(not(windows))]
pub fn acquire_instance() -> Result<Option<InstanceGuard>, String> {
    STARTED.get_or_init(Instant::now);
    Ok(Some(InstanceGuard))
}

/// The user completes any removal in Windows; this command never deletes files or launches an uninstaller.
#[tauri::command]
pub async fn open_uninstall_settings() -> Result<String, String> {
    #[cfg(windows)]
    {
        let uri = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from("ms-settings:appsfeatures"))
            .map_err(|_| "Could not open Windows Installed apps. Open Windows Settings → Apps → Installed apps manually.".to_string())?;
        let launched = windows::System::Launcher::LaunchUriAsync(&uri)
            .map_err(|_| "Could not open Windows Installed apps. Open Windows Settings → Apps → Installed apps manually.".to_string())?
            .await
            .map_err(|_| "Could not open Windows Installed apps. Open Windows Settings → Apps → Installed apps manually.".to_string())?;
        if !launched {
            return Err("Windows could not open Installed apps. Open Windows Settings → Apps → Installed apps manually.".into());
        }
        Ok("Windows Installed apps opened. Search for sudo-island and choose Uninstall. If Sudo is not listed because you use a portable copy, close Sudo and delete that copy manually.".into())
    }
    #[cfg(not(windows))]
    {
        Err("Open your system's application manager to uninstall Sudo.".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::InstanceGuard;

    #[test]
    fn relaunches_are_spaced_out_and_stop_after_a_few_in_a_row() {
        use super::next_recovery;
        use std::time::Duration;
        let minute = Duration::from_secs(60);
        assert_eq!(next_recovery(Duration::from_secs(44), 0), None);
        assert_eq!(next_recovery(minute, 0), Some(1));
        assert_eq!(next_recovery(minute, 2), Some(3));
        assert_eq!(next_recovery(minute, 3), None);
        // A copy that ran for a long time was a successful recovery; the count starts again.
        assert_eq!(next_recovery(minute * 10, 3), Some(1));
    }

    #[test]
    fn reads_how_many_relaunches_led_to_this_copy() {
        use super::recovery_count;
        let arguments = |list: &[&str]| list.iter().map(|argument| argument.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(recovery_count(arguments(&["sudo-island.exe"])), 0);
        assert_eq!(recovery_count(arguments(&["sudo-island.exe", "--recover-graphics"])), 1);
        assert_eq!(recovery_count(arguments(&["sudo-island.exe", "--recover-graphics=2"])), 2);
        assert_eq!(recovery_count(arguments(&["sudo-island.exe", "--recover-graphics=x"])), 1);
    }

    #[test]
    fn named_instance_exists_until_its_last_guard_is_dropped() {
        // An isolated kernel-object name avoids interfering with a running Sudo.
        let name = format!(r"Local\dev.sudo.island.lifecycle-test.{}", std::process::id());
        let first = InstanceGuard::acquire_named(&name).unwrap().expect("first instance");
        assert!(InstanceGuard::acquire_named(&name).unwrap().is_none());
        // Rejecting duplicates must not close the original instance's handle.
        assert!(InstanceGuard::acquire_named(&name).unwrap().is_none());
        drop(first);
        assert!(InstanceGuard::acquire_named(&name).unwrap().is_some());
    }
}
