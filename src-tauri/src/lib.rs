//! Carrier — a tiny, distraction-free desktop client for Facebook Messenger.
//!
//! Opens a WebView window pointed at the Messenger web app, injects a stylesheet
//! that hides Facebook's surrounding chrome, and adds quality-of-life features:
//! shortcuts, zoom, an image viewer, a settings panel, copy/download image,
//! native notifications, theme sync, and tracking-redirect-free external links.
//! Anything that isn't Messenger is handed to the user's default browser.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{AboutMetadata, Menu, MenuItem, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    webview::{Color, DownloadEvent},
    Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;
use url::Url;

/// The page we wrap.
const HOME_URL: &str = "https://www.facebook.com/messages";

/// Injected assets (clean-room; see `inject/`).
const INJECT_CSS: &str = include_str!("../inject/messenger.css");
const INJECT_JS: &str = include_str!("../inject/messenger.js");
const INJECT_PANEL: &str = include_str!("../inject/panel.js");

/// A modern browser UA so Facebook serves the full Messenger web app.
const fn user_agent() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/17.4 Safari/605.1.15"
    }
    #[cfg(target_os = "windows")]
    {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
    }
    #[cfg(target_os = "linux")]
    {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    always_on_top: bool,
    show_tray: bool,
    start_to_tray: bool,
    autostart: bool,
    hide_on_close: bool,
    /// Experimental: when true, single-instance enforcement is skipped at the
    /// next launch (takes effect after restart).
    multi_instance: bool,
    spellcheck: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            show_tray: true,
            start_to_tray: false,
            autostart: false,
            hide_on_close: true,
            multi_instance: false,
            spellcheck: true,
        }
    }
}

struct AppState {
    settings: Mutex<Settings>,
    tray: Mutex<Option<TrayIcon>>,
    next_window: AtomicUsize,
}

const APP_IDENTIFIER: &str = "io.github.kristofferr.carrier";

fn settings_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("settings.json"))
}

fn load_settings(app: &tauri::AppHandle) -> Settings {
    settings_file(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Read persisted settings directly from disk before the Tauri app is built
/// (used to decide single-instance enforcement). Falls back to defaults.
fn load_settings_early() -> Settings {
    dirs_config_dir()
        .map(|b| b.join(APP_IDENTIFIER).join("settings.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
            })
    }
}

fn save_settings(app: &tauri::AppHandle, s: &Settings) -> Result<(), String> {
    let path = settings_file(app).ok_or("no config directory available")?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tray
// ---------------------------------------------------------------------------

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<TrayIcon> {
    let show_item = MenuItem::with_id(app, "show", "Open Carrier", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Carrier", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    TrayIconBuilder::with_id("carrier-tray")
        .tooltip("Carrier")
        .icon(app.default_window_icon().expect("bundled icon").clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)
}

/// Register or unregister Start on System Startup with the OS. Kept separate so
/// callers can sync it *before* persisting and avoid committing a preference the
/// OS rejected.
fn sync_autostart(app: &tauri::AppHandle, want: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    let res = if want { mgr.enable() } else { mgr.disable() };
    res.map_err(|e| format!("Couldn't update Start on System Startup: {e}"))
}

/// Apply the settings that have an immediate runtime effect (window topmost
/// state, the injected-prefs refresh, and the tray). Autostart is handled
/// separately by [`sync_autostart`]; everything here is best-effort.
fn apply_settings(app: &tauri::AppHandle, s: &Settings) {
    let settings_json = serde_json::to_string(s).ok();
    for (label, window) in app.webview_windows() {
        // Apply to every window (incl. the Settings dialog) so toggling Always
        // on Top from the dialog doesn't leave the dialog stuck behind the
        // now-topmost Messenger windows.
        let _ = window.set_always_on_top(s.always_on_top);
        if label != "settings" {
            // Push the new prefs to the running page so JS-side settings
            // (spell-check) refresh without a reload.
            if let Some(ref json) = settings_json {
                let _ = window.eval(format!(
                    "window.__CARRIER_SETTINGS__ = {json}; window.dispatchEvent(new Event('carrier:settings'));"
                ));
            }
        }
    }

    // Tray: create or tear down to match `show_tray`.
    let state = app.state::<AppState>();
    let mut tray = state.tray.lock().unwrap();
    match (s.show_tray, tray.is_some()) {
        (true, false) => {
            if let Ok(t) = build_tray(app) {
                *tray = Some(t);
            }
        }
        (false, true) => {
            // Removing the only way back, so make sure the main window is
            // visible before dropping the tray icon.
            show_main(app);
            // `build()` also registers a clone in Tauri's resource table, so
            // dropping our handle alone leaves the icon visible — remove it by id.
            let _ = app.remove_tray_by_id("carrier-tray");
            *tray = None;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Link handling
// ---------------------------------------------------------------------------

/// Facebook wraps external links in tracking redirects
/// (`l.facebook.com/l.php?u=…`, `lm.facebook.com/l.php?u=…`, `facebook.com/l.php`).
/// Return the real destination if `url` is such a redirect.
fn unwrap_tracking(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let is_redirect = host == "l.facebook.com"
        || host == "lm.facebook.com"
        || (host == "facebook.com" && url.path() == "/l.php");
    if !is_redirect {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "u")
        .map(|(_, v)| v.into_owned())
        // Only unwrap to a real web URL — never a javascript:/file:/data: target
        // smuggled through the `u=` parameter.
        .filter(|target| {
            Url::parse(target)
                .map(|u| matches!(u.scheme(), "http" | "https"))
                .unwrap_or(false)
        })
}

/// OAuth/login URLs that must stay *inside* the app, so Facebook's "continue
/// with Google/Apple/Microsoft" social logins work as an in-app popup instead of
/// bouncing to the browser. Restricted to the dedicated auth hosts (which serve
/// nothing but auth) — matching on OAuth *paths* across arbitrary hosts is both
/// unnecessary (Facebook doesn't offer those providers) and error-prone.
fn is_auth_url(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    const AUTH_HOSTS: &[&str] = &[
        "accounts.google.com",
        "login.microsoftonline.com",
        "appleid.apple.com",
    ];
    AUTH_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// Domains kept *inside* the app (Messenger plus the Facebook/Meta auth and
/// media surfaces needed to log in and load content).
fn is_internal(url: &Url) -> bool {
    match url.scheme() {
        "about" => return true,
        // Resolve a blob: URL to its inner origin and judge that.
        "blob" => {
            return url
                .as_str()
                .strip_prefix("blob:")
                .and_then(|inner| Url::parse(inner).ok())
                .is_some_and(|inner| is_internal(&inner));
        }
        // data: / javascript: (and anything else) are never "internal".
        "http" | "https" => {}
        _ => return false,
    }
    if is_auth_url(url) {
        return true;
    }
    // Reject hostless HTTP(S) rather than treating it as internal.
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    const INTERNAL_SUFFIXES: &[&str] = &[
        "facebook.com",
        "messenger.com",
        "fbcdn.net",
        "fbsbx.com",
        "meta.com",
        "oculus.com",
    ];
    INTERNAL_SUFFIXES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

// ---------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------

/// Only the commands that fetch a URL (copy/download image & video) are exposed
/// to the remote page, so restrict them to Facebook/Messenger media hosts over
/// HTTPS. This is a strict allowlist — far stronger than IP filtering, since an
/// attacker can't point `fbcdn.net` at a private/loopback address via DNS
/// rebinding to reach the local network (SSRF).
fn is_fetchable_media_host(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    const HOSTS: &[&str] = &["fbcdn.net", "fbsbx.com", "facebook.com", "messenger.com"];
    HOSTS
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// Fetch a Facebook/Messenger media URL into memory (capped), with timeouts and
/// no redirects.
fn fetch_public(url: &str, cap: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let parsed = Url::parse(url).map_err(|e| e.to_string())?;
    if !is_fetchable_media_host(&parsed) {
        return Err("refusing to fetch a non-Messenger URL".into());
    }
    // No redirects: the allowlisted host must not be able to bounce us elsewhere
    // (SSRF). Accept 2xx only.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(60))
        .redirects(0)
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("fetch failed: {e}"))?;
    if resp.status() >= 300 {
        return Err("refusing to follow a redirect".into());
    }
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 >= cap {
        return Err("response too large".into());
    }
    Ok(bytes)
}

fn downloads_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("HOME").map(std::path::PathBuf::from);
    base.map(|b| b.join("Downloads"))
}

/// Minimal percent-decoder for filenames.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reduce a filename to a single safe path component so it can't escape the
/// Downloads folder (path separators, NUL, and Windows drive/reserved chars).
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | '\0' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
            ) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned.to_string()
    }
}

/// Best-effort filename from a URL: last path segment, percent-decoded, query
/// stripped. Keeps the URL's own extension (so a video isn't saved as `.png`).
fn filename_from_url(url: &Url) -> String {
    let raw = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    sanitize_filename(&percent_decode(raw))
}

/// Avoid clobbering an existing file by appending " (n)".
fn unique_path(p: std::path::PathBuf) -> std::path::PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let parent = p.parent().map(|x| x.to_path_buf()).unwrap_or_default();
    for n in 1..10000 {
        let cand = parent.join(format!("{stem} ({n}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    p
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// Persist `new`, syncing the OS autostart registration first so a failed sync
/// doesn't commit an autostart preference the OS rejected. Other preferences are
/// always saved; an autostart failure is returned (after saving the rest) so the
/// UI can surface it.
fn store_settings(
    app: &tauri::AppHandle,
    state: &State<AppState>,
    new: Settings,
) -> Result<Settings, String> {
    let prev_autostart = state.settings.lock().unwrap().autostart;
    let mut effective = new.clone();
    let autostart_err = if new.autostart != prev_autostart {
        match sync_autostart(app, new.autostart) {
            Ok(()) => None,
            // Keep the previous autostart value rather than persisting one the OS
            // didn't accept; still save/apply every other preference.
            Err(e) => {
                effective.autostart = prev_autostart;
                Some(e)
            }
        }
    } else {
        None
    };
    save_settings(app, &effective)?;
    *state.settings.lock().unwrap() = effective.clone();
    apply_settings(app, &effective);
    match autostart_err {
        Some(e) => Err(e),
        None => Ok(effective),
    }
}

#[tauri::command]
fn set_settings(
    app: tauri::AppHandle,
    state: State<AppState>,
    new: Settings,
) -> Result<Settings, String> {
    store_settings(&app, &state, new)
}

/// Reset all settings to their defaults.
#[tauri::command]
fn reset_settings(app: tauri::AppHandle, state: State<AppState>) -> Result<Settings, String> {
    store_settings(&app, &state, Settings::default())
}

/// Open a URL in the user's default browser, unwrapping FB tracking redirects.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    let target = unwrap_tracking(&parsed).unwrap_or(url);
    // Only hand safe web schemes to the OS opener so a page script can't ask us
    // to open arbitrary `file://`/custom-scheme URIs.
    let scheme = Url::parse(&target)
        .map(|u| u.scheme().to_string())
        .unwrap_or_default();
    if !matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel") {
        return Err(format!("refusing to open non-web URL ({scheme})"));
    }
    open::that(target).map_err(|e| e.to_string())
}

/// Download an image and place it on the system clipboard.
#[tauri::command]
fn copy_image(url: String) -> Result<(), String> {
    let bytes = fetch_public(&url, 40 * 1024 * 1024)?;
    // Cap dimensions/allocation during decode so a small but highly compressed
    // image (a decompression bomb) can't blow up memory in `to_rgba8()`.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("decode failed: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();

    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: std::borrow::Cow::Owned(img.into_raw()),
        })
        .map_err(|e| e.to_string())
}

/// Download a URL to the Downloads folder; returns the saved path. Async +
/// `spawn_blocking` so the blocking fetch/copy of a large file doesn't run on the
/// main thread (which would freeze the UI).
#[tauri::command]
async fn download_file(url: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_file_blocking(url))
        .await
        .map_err(|e| e.to_string())?
}

fn download_file_blocking(url: String) -> Result<String, String> {
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    if !is_fetchable_media_host(&parsed) {
        return Err("refusing to fetch a non-Messenger URL".into());
    }
    let dir = downloads_dir().ok_or("no downloads directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = unique_path(dir.join(filename_from_url(&parsed)));

    // Connect + read timeouts (the read timeout fires only when a stalled
    // connection sends nothing, so it doesn't abort a slow-but-progressing
    // download) and no redirects (SSRF).
    use std::io::Read;
    const CAP: u64 = 2 * 1024 * 1024 * 1024; // 2 GB
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(120))
        .redirects(0)
        .build();
    let resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    if resp.status() >= 300 {
        return Err("refusing to follow a redirect".into());
    }
    let mut reader = resp.into_reader().take(CAP);
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    // Don't leave a partial (errored) or oversized file behind.
    match std::io::copy(&mut reader, &mut file) {
        Ok(n) if n < CAP => {}
        other => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(match other {
                Ok(_) => "file too large".to_string(),
                Err(e) => e.to_string(),
            });
        }
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Save base64-encoded bytes (e.g. a `blob:`/`data:` image the page rendered
/// but that isn't a plain downloadable URL) to the Downloads folder. Async +
/// `spawn_blocking` to keep the base64 decode + write off the main thread.
#[tauri::command]
async fn download_file_by_binary(filename: String, data: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_by_binary_blocking(filename, data))
        .await
        .map_err(|e| e.to_string())?
}

fn download_by_binary_blocking(filename: String, data: String) -> Result<String, String> {
    // Cap the decoded size (~3/4 of the base64 length) before allocating.
    const MAX_BYTES: usize = 512 * 1024 * 1024;
    if data.len() / 4 * 3 > MAX_BYTES {
        return Err("file too large".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| e.to_string())?;
    let dir = downloads_dir().ok_or("no downloads directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = unique_path(dir.join(sanitize_filename(&filename)));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Show a native OS notification (used by the injected Web Notification shim).
#[tauri::command]
fn send_notification(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

/// Sync the native window theme with the page's `prefers-color-scheme`.
#[tauri::command]
fn update_theme_mode(app: tauri::AppHandle, mode: String) -> Result<(), String> {
    let theme = match mode.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None, // follow the system
    };
    for w in app.webview_windows().values() {
        let _ = w.set_theme(theme);
    }
    Ok(())
}

/// Open the OS privacy settings for camera/microphone (when a call is blocked).
#[tauri::command]
fn open_privacy_settings(kind: Option<String>) -> Result<(), String> {
    let mic = kind.as_deref() == Some("microphone");
    let _ = mic;
    #[cfg(target_os = "macos")]
    let url = if mic {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
    } else {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
    };
    #[cfg(target_os = "windows")]
    let url = if mic {
        "ms-settings:privacy-microphone"
    } else {
        "ms-settings:privacy-webcam"
    };
    #[cfg(target_os = "linux")]
    return Ok(());
    #[cfg(not(target_os = "linux"))]
    open::that(url).map_err(|e| e.to_string())
}

#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// Clear the WebView's cookies/cache/storage, then relaunch.
fn clear_cache(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.clear_all_browsing_data();
    }
    if let Ok(cache) = app.path().app_cache_dir() {
        let _ = std::fs::remove_dir_all(&cache);
    }
}

#[tauri::command]
fn clear_cache_and_restart(app: tauri::AppHandle) {
    clear_cache(&app);
    app.restart();
}

/// Check GitHub releases for an update; download & install if found.
#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => {
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| e.to_string())?;
            app.restart();
        }
        Ok(None) => Ok("up-to-date".into()),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// A theme-appropriate window background so there's no white flash before the
/// remote page paints (Facebook glares white in dark mode while loading).
fn splash_background() -> Color {
    if matches!(dark_light::detect(), dark_light::Mode::Dark) {
        Color(24, 25, 26, 255) // Facebook dark
    } else {
        Color(255, 255, 255, 255)
    }
}

/// Build a Carrier window (used for the main window and any extra windows).
fn build_app_window(
    app: &tauri::AppHandle,
    label: &str,
    settings: &Settings,
) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::External(HOME_URL.parse().expect("valid home URL")),
    )
    .title("Carrier")
    .inner_size(1200.0, 780.0)
    .min_inner_size(420.0, 520.0)
    .background_color(splash_background())
    .user_agent(user_agent())
    .initialization_script(init_script(settings))
    .on_navigation(|url| {
        // External tracking redirect -> open the real (web-only) destination.
        if let Some(real) = unwrap_tracking(url) {
            if Url::parse(&real).is_ok_and(|r| matches!(r.scheme(), "http" | "https")) {
                let _ = open::that(real);
            }
            return false;
        }
        if is_internal(url) {
            return true;
        }
        // Open ordinary web links in the browser; block anything else
        // (data:, javascript:, file:, custom schemes).
        if matches!(url.scheme(), "http" | "https") {
            let _ = open::that(url.as_str());
        }
        false
    })
    .on_download(|_webview, event| {
        if let DownloadEvent::Requested { url, destination } = event {
            // Only accept downloads of Messenger's own media or page-generated
            // blob:/data: content; refuse anything else a remote page might try
            // to write to the user's Downloads folder.
            let allowed = matches!(url.scheme(), "blob" | "data") || is_fetchable_media_host(&url);
            if !allowed {
                return false;
            }
            if let Some(dir) = downloads_dir() {
                let _ = std::fs::create_dir_all(&dir);
                *destination = unique_path(dir.join(filename_from_url(&url)));
            }
        }
        true
    })
    .build()
    .inspect(|window| {
        // New windows inherit the current always-on-top preference.
        let _ = window.set_always_on_top(settings.always_on_top);
    })
}

/// Open (or focus) the dedicated settings window (a small local page, separate
/// from the Messenger view).
fn show_settings_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // Match the Messenger windows' topmost state so the dialog isn't trapped
    // behind them when Always on Top is enabled.
    let aot = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .always_on_top;
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Carrier Settings")
        .inner_size(460.0, 620.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(aot)
        .build();
}

#[tauri::command]
fn open_settings_window(app: tauri::AppHandle) {
    // Build the window off the synchronous command handler —
    // `WebviewWindowBuilder::new` can deadlock on Windows otherwise.
    tauri::async_runtime::spawn(async move {
        show_settings_window(&app);
    });
}

// ---------------------------------------------------------------------------
// Native menu
// ---------------------------------------------------------------------------

fn build_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let mi = |id: &str, label: &str, accel: Option<&str>| -> tauri::Result<MenuItem<tauri::Wry>> {
        let mut b = MenuItemBuilder::new(label).id(id);
        if let Some(a) = accel {
            b = b.accelerator(a);
        }
        b.build(app)
    };

    let prefs = mi("preferences", "Settings…", Some("CmdOrCtrl+,"))?;
    let app_menu = SubmenuBuilder::new(app, "Carrier")
        .about(Some(AboutMetadata::default()))
        .separator()
        .item(&prefs)
        .separator()
        .hide()
        .separator()
        .quit()
        .build()?;

    let new_window = mi("new_window", "New Window", Some("CmdOrCtrl+N"))?;
    let print = mi("print", "Print…", Some("CmdOrCtrl+P"))?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&new_window)
        .separator()
        .item(&print)
        .separator()
        .close_window()
        .build()?;

    let paste_match = mi(
        "paste_match_style",
        "Paste and Match Style",
        Some("CmdOrCtrl+Shift+Alt+V"),
    )?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .item(&paste_match)
        .select_all()
        .build()?;

    let reload = mi("reload", "Reload", Some("CmdOrCtrl+R"))?;
    let clear_cache = mi(
        "clear_cache",
        "Clear Cache & Restart",
        Some("CmdOrCtrl+Shift+Backspace"),
    )?;
    let zreset = mi("zoom_reset", "Actual Size", Some("CmdOrCtrl+0"))?;
    let zin = mi("zoom_in", "Zoom In", Some("CmdOrCtrl+="))?;
    let zout = mi("zoom_out", "Zoom Out", Some("CmdOrCtrl+-"))?;
    let aot = mi("always_on_top", "Toggle Always on Top", None)?;
    let devtools = mi(
        "devtools",
        "Toggle Developer Tools",
        Some("CmdOrCtrl+Alt+I"),
    )?;
    let view = {
        let b = SubmenuBuilder::new(app, "View")
            .item(&reload)
            .item(&clear_cache)
            .separator()
            .item(&zreset)
            .item(&zin)
            .item(&zout)
            .separator()
            .item(&aot);
        #[cfg(debug_assertions)]
        let b = b.separator().item(&devtools);
        let _ = &devtools;
        b.build()?
    };

    let back = mi("back", "Back", Some("CmdOrCtrl+["))?;
    let fwd = mi("forward", "Forward", Some("CmdOrCtrl+]"))?;
    let home = mi("home", "Home", Some("CmdOrCtrl+Shift+H"))?;
    let copy_url = mi("copy_url", "Copy Current URL", None)?;
    let history = SubmenuBuilder::new(app, "History")
        .item(&back)
        .item(&fwd)
        .separator()
        .item(&home)
        .item(&copy_url)
        .build()?;

    let maximize = mi("maximize", "Zoom", None)?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .item(&maximize)
        .separator()
        .close_window()
        .build()?;

    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &history, &window])
}

/// The focused Messenger window (a `main`/`win-*` window), falling back to
/// `main`. Used so menu actions affect the window the user is actually looking
/// at rather than always `main`. The local settings window is excluded.
fn target_window(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .find(|(label, w)| label.as_str() != "settings" && w.is_focused().unwrap_or(false))
        .map(|(_, w)| w)
        .or_else(|| app.get_webview_window("main"))
}

fn handle_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    let eval = |js: &str| {
        if let Some(w) = target_window(app) {
            let _ = w.eval(js);
        }
    };
    match event.id().as_ref() {
        "preferences" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { show_settings_window(&app) });
        }
        "reload" => eval("location.reload()"),
        "back" => eval("history.back()"),
        "forward" => eval("history.forward()"),
        "home" => eval(&format!("location.assign('{HOME_URL}')")),
        "zoom_in" => eval("window.__carrierZoomIn && window.__carrierZoomIn()"),
        "zoom_out" => eval("window.__carrierZoomOut && window.__carrierZoomOut()"),
        "zoom_reset" => eval("window.__carrierZoomReset && window.__carrierZoomReset()"),
        "copy_url" => eval(
            "navigator.clipboard && navigator.clipboard.writeText(location.href); \
             window.__carrierToast && window.__carrierToast('Link copied')",
        ),
        "paste_match_style" => eval(
            "navigator.clipboard && navigator.clipboard.readText().then(function (t) { \
             document.execCommand('insertText', false, t); })",
        ),
        "print" => {
            if let Some(w) = target_window(app) {
                let _ = w.print();
            }
        }
        "maximize" => {
            if let Some(w) = target_window(app) {
                if w.is_maximized().unwrap_or(false) {
                    let _ = w.unmaximize();
                } else {
                    let _ = w.maximize();
                }
            }
        }
        "new_window" => {
            // Off the event-loop handler to avoid the Windows window-creation
            // deadlock.
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let s = app.state::<AppState>().settings.lock().unwrap().clone();
                let n = app
                    .state::<AppState>()
                    .next_window
                    .fetch_add(1, Ordering::SeqCst);
                let _ = build_app_window(&app, &format!("win-{n}"), &s);
            });
        }
        "clear_cache" => {
            clear_cache(app);
            app.restart();
        }
        "always_on_top" => {
            let state = app.state::<AppState>();
            let mut s = state.settings.lock().unwrap().clone();
            s.always_on_top = !s.always_on_top;
            *state.settings.lock().unwrap() = s.clone();
            if let Err(e) = save_settings(app, &s) {
                eprintln!("carrier: failed to save settings: {e}");
            }
            for (label, w) in app.webview_windows() {
                if label != "settings" {
                    let _ = w.set_always_on_top(s.always_on_top);
                }
            }
        }
        "devtools" =>
        {
            #[cfg(debug_assertions)]
            if let Some(w) = app.get_webview_window("main") {
                w.open_devtools();
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

fn init_script(settings: &Settings) -> String {
    let css_literal = serde_json::to_string(INJECT_CSS).expect("CSS serialises");
    let settings_literal = serde_json::to_string(settings).expect("settings serialise");
    format!(
        r#"(function () {{
  window.__CARRIER_SETTINGS__ = {settings_literal};
  var css = {css_literal};
  function inject() {{
    if (!document.head) return false;
    if (document.head.querySelector('style[data-carrier]')) return true;
    var s = document.createElement('style');
    s.setAttribute('data-carrier', '');
    s.textContent = css;
    document.head.appendChild(s);
    return true;
  }}
  if (!inject()) {{
    new MutationObserver(function (_, obs) {{ if (inject()) obs.disconnect(); }})
      .observe(document.documentElement, {{ childList: true, subtree: true }});
  }}
}})();
{INJECT_JS}
{INJECT_PANEL}"#
    )
}

pub fn run() {
    let initial = load_settings_early();

    let mut builder = tauri::Builder::default();

    // Single-instance enforcement (unless the experimental multi-instance flag
    // is set). Must be registered first so it runs before any window is created.
    if !initial.multi_instance {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }));
    }

    builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Persist geometry only — NOT visibility, so the app always shows
                // its window on launch (unless Start to Tray) rather than coming
                // back hidden after a previous hide-to-tray.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .with_denylist(&["settings"]) // fixed-size dialog; don't persist its geometry
                .build(),
        )
        .plugin(tauri_plugin_log::Builder::new().build())
        .manage(AppState {
            settings: Mutex::new(initial.clone()),
            tray: Mutex::new(None),
            next_window: AtomicUsize::new(2),
        })
        .menu(build_menu)
        .on_menu_event(handle_menu_event)
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            reset_settings,
            open_external,
            open_settings_window,
            copy_image,
            download_file,
            download_file_by_binary,
            send_notification,
            update_theme_mode,
            open_privacy_settings,
            restart_app,
            clear_cache_and_restart,
            check_for_updates
        ])
        .setup(move |app| {
            let settings = load_settings(app.handle());
            *app.state::<AppState>().settings.lock().unwrap() = settings.clone();

            let window = build_app_window(app.handle(), "main", &settings)?;

            // Close button: hide to tray (if enabled) instead of quitting.
            let handle = app.handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    let (hide, has_tray) = {
                        let state = handle.state::<AppState>();
                        let hide = state.settings.lock().unwrap().hide_on_close;
                        let has_tray = state.tray.lock().unwrap().is_some();
                        (hide, has_tray)
                    };
                    // Only hide to the tray if one was actually created (tray
                    // creation can fail, e.g. on a Linux session without an
                    // AppIndicator); otherwise closing the main window quits the
                    // app (don't let an open Settings dialog keep it running).
                    if hide && has_tray {
                        api.prevent_close();
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    } else {
                        handle.exit(0);
                    }
                }
            });

            // Don't sync autostart at startup; the OS registration already
            // reflects the user's last explicit choice.
            apply_settings(app.handle(), &settings);

            // Start hidden only when a tray was actually created to reopen from.
            let has_tray = app.state::<AppState>().tray.lock().unwrap().is_some();
            if settings.start_to_tray && has_tray {
                let _ = window.hide();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Carrier");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn fetchable_media_host_is_an_https_fb_allowlist() {
        assert!(is_fetchable_media_host(&u(
            "https://scontent.fbcdn.net/v/x.jpg"
        )));
        assert!(is_fetchable_media_host(&u(
            "https://video.xx.fbcdn.net/x.mp4"
        )));
        assert!(is_fetchable_media_host(&u("https://www.facebook.com/x")));
        // wrong scheme, arbitrary hosts, and IPs are all rejected
        assert!(!is_fetchable_media_host(&u(
            "http://scontent.fbcdn.net/x.jpg"
        ))); // not https
        assert!(!is_fetchable_media_host(&u("https://example.com/x.jpg")));
        assert!(!is_fetchable_media_host(&u("https://evil-fbcdn.net/x"))); // suffix trick
        assert!(!is_fetchable_media_host(&u("https://127.0.0.1/x")));
        assert!(!is_fetchable_media_host(&u("https://localhost/x")));
    }

    #[test]
    fn internal_allows_messenger_blocks_dangerous() {
        assert!(is_internal(&u("https://www.facebook.com/messages")));
        assert!(is_internal(&u("https://web.facebook.com/x")));
        assert!(is_internal(&u("https://accounts.google.com/o/oauth2/auth")));
        assert!(is_internal(&u("about:blank")));
        assert!(!is_internal(&u("https://example.com/")));
        assert!(!is_internal(&u("data:text/html,<script>1</script>")));
        assert!(!is_internal(&u("javascript:alert(1)")));
    }

    #[test]
    fn auth_is_dedicated_hosts_only() {
        assert!(is_auth_url(&u("https://accounts.google.com/anything")));
        assert!(is_auth_url(&u("https://appleid.apple.com/auth/authorize")));
        assert!(is_auth_url(&u(
            "https://login.microsoftonline.com/common/oauth2"
        )));
        // code hosts and arbitrary /oauth paths are external, not in-app auth
        assert!(!is_auth_url(&u("https://github.com/login/oauth/authorize")));
        assert!(!is_auth_url(&u("https://github.com/user/repo")));
        assert!(!is_auth_url(&u("https://example.com/oauth/authorize")));
    }

    #[test]
    fn sanitize_blocks_traversal_and_windows_drive() {
        let a = sanitize_filename("../../etc/passwd");
        assert!(!a.contains('/') && !a.contains('\\'));
        let b = sanitize_filename("C:evil.exe");
        assert!(!b.contains(':'));
        assert_eq!(sanitize_filename("   "), "download");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename("photo.png"), "photo.png");
    }

    #[test]
    fn tracking_redirect_is_unwrapped() {
        let url = u("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.com%2Fa&h=AT0");
        assert_eq!(
            unwrap_tracking(&url).as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            unwrap_tracking(&u("https://www.facebook.com/messages")),
            None
        );
        // A tracking redirect whose `u=` target is a non-HTTP(S) scheme must not
        // be unwrapped (defense-in-depth against javascript:/file:/data:).
        assert_eq!(
            unwrap_tracking(&u(
                "https://l.facebook.com/l.php?u=javascript%3Aalert%281%29&h=AT0"
            )),
            None
        );
        assert_eq!(
            unwrap_tracking(&u(
                "https://l.facebook.com/l.php?u=file%3A%2F%2F%2Fetc%2Fpasswd"
            )),
            None
        );
    }

    #[test]
    fn filename_keeps_real_extension() {
        assert_eq!(
            filename_from_url(&u("https://x.com/a/video.mp4?dl=1")),
            "video.mp4"
        );
        assert_eq!(filename_from_url(&u("https://x.com/")), "download");
    }
}
