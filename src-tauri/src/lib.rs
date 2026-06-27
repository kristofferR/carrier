//! Carrier — a tiny, distraction-free desktop client for Facebook Messenger.
//!
//! Opens a WebView window pointed at the Messenger web app, injects a stylesheet
//! that hides Facebook's surrounding chrome, and adds quality-of-life features:
//! shortcuts, zoom, an image viewer, a settings panel, copy/download image,
//! native notifications, theme sync, and tracking-redirect-free external links.
//! Anything that isn't Messenger is handed to the user's default browser.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

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
/// Test-only bridge so `tauri-plugin-mcp`'s `execute_js` works on the remote page.
#[cfg(feature = "mcp")]
const MCP_TEST_LISTENER: &str = include_str!("../inject/mcp_listener.js");

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
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    }
}

fn save_settings(app: &tauri::AppHandle, s: &Settings) {
    if let Some(path) = settings_file(app) {
        if let Ok(json) = serde_json::to_string_pretty(s) {
            let _ = std::fs::write(path, json);
        }
    }
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

/// Apply settings that have an immediate runtime effect.
fn apply_settings(app: &tauri::AppHandle, s: &Settings) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(s.always_on_top);
    }

    // Autostart (Start on System Startup).
    let mgr = app.autolaunch();
    if s.autostart {
        let _ = mgr.enable();
    } else {
        let _ = mgr.disable();
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
            *tray = None; // dropping the TrayIcon removes it
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
}

/// OAuth/login URLs that must stay *inside* the app (so logging in with
/// Google/Apple/Microsoft/GitHub works in a popup instead of the browser).
fn is_auth_url(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("").to_lowercase();
    let path = url.path().to_lowercase();
    const HOSTS: &[&str] = &[
        "accounts.google.com",
        "login.microsoftonline.com",
        "appleid.apple.com",
        "github.com",
    ];
    if HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) {
        return true;
    }
    const PATHS: &[&str] = &["/oauth", "/o/oauth2", "/authorize", "/signin", "/login", "/dialog/oauth"];
    PATHS.iter().any(|p| path.contains(p))
}

/// Domains kept *inside* the app (Messenger plus the Facebook/Meta auth and
/// media surfaces needed to log in and load content).
fn is_internal(url: &Url) -> bool {
    match url.scheme() {
        "about" | "blob" | "data" | "javascript" => return true,
        "http" | "https" => {}
        _ => return false,
    }
    if is_auth_url(url) {
        return true;
    }
    let Some(host) = url.host_str() else { return true };
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

/// Strip path separators so a filename can't escape the Downloads folder.
fn sanitize_filename(name: &str) -> String {
    let cleaned = name.replace(['/', '\\', '\0'], "_");
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
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("download").to_string();
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

#[tauri::command]
fn set_settings(app: tauri::AppHandle, state: State<AppState>, new: Settings) -> Settings {
    {
        let mut guard = state.settings.lock().unwrap();
        *guard = new.clone();
    }
    save_settings(&app, &new);
    apply_settings(&app, &new);
    new
}

/// Open a URL in the user's default browser, unwrapping FB tracking redirects.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let target = Url::parse(&url)
        .ok()
        .and_then(|u| unwrap_tracking(&u))
        .unwrap_or(url);
    open::that(target).map_err(|e| e.to_string())
}

/// Download an image and place it on the system clipboard.
#[tauri::command]
fn copy_image(url: String) -> Result<(), String> {
    use std::io::Read;
    let resp = ureq::get(&url).call().map_err(|e| format!("fetch failed: {e}"))?;
    let mut bytes: Vec<u8> = Vec::new();
    resp.into_reader()
        .take(40 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;

    let img = image::load_from_memory(&bytes)
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

/// Download a URL to the Downloads folder; returns the saved path.
#[tauri::command]
fn download_file(url: String) -> Result<String, String> {
    use std::io::Read;
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    let dir = downloads_dir().ok_or("no downloads directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = unique_path(dir.join(filename_from_url(&parsed)));

    let resp = ureq::get(&url).call().map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader().take(500 * 1024 * 1024);
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Save base64-encoded bytes (e.g. a `blob:`/`data:` image the page rendered
/// but that isn't a plain downloadable URL) to the Downloads folder.
#[tauri::command]
fn download_file_by_binary(filename: String, data: String) -> Result<String, String> {
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

#[tauri::command]
fn clear_cache_and_restart(app: tauri::AppHandle) {
    if let Ok(cache) = app.path().app_cache_dir() {
        let _ = std::fs::remove_dir_all(&cache);
    }
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
    .initialization_script(&init_script(settings))
    .on_navigation(|url| {
        if let Some(real) = unwrap_tracking(url) {
            let _ = open::that(real);
            return false;
        }
        if is_internal(url) {
            true
        } else {
            let _ = open::that(url.as_str());
            false
        }
    })
    .on_download(|_webview, event| {
        if let DownloadEvent::Requested { url, destination } = event {
            if let Some(dir) = downloads_dir() {
                let _ = std::fs::create_dir_all(&dir);
                *destination = unique_path(dir.join(filename_from_url(&url)));
            }
        }
        true
    })
    .build()
}

/// Open (or focus) the dedicated settings window (a small local page, separate
/// from the Messenger view).
fn show_settings_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Carrier Settings")
        .inner_size(460.0, 620.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .build();
}

#[tauri::command]
fn open_settings_window(app: tauri::AppHandle) {
    show_settings_window(&app);
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

    let paste_match = mi("paste_match_style", "Paste and Match Style", Some("CmdOrCtrl+Shift+Alt+V"))?;
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
    let clear_cache = mi("clear_cache", "Clear Cache & Restart", Some("CmdOrCtrl+Shift+Backspace"))?;
    let zreset = mi("zoom_reset", "Actual Size", Some("CmdOrCtrl+0"))?;
    let zin = mi("zoom_in", "Zoom In", Some("CmdOrCtrl+="))?;
    let zout = mi("zoom_out", "Zoom Out", Some("CmdOrCtrl+-"))?;
    let aot = mi("always_on_top", "Toggle Always on Top", None)?;
    let devtools = mi("devtools", "Toggle Developer Tools", Some("CmdOrCtrl+Alt+I"))?;
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

fn handle_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    let eval = |js: &str| {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.eval(js);
        }
    };
    match event.id().as_ref() {
        "preferences" => show_settings_window(app),
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
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.print();
            }
        }
        "maximize" => {
            if let Some(w) = app.get_webview_window("main") {
                if w.is_maximized().unwrap_or(false) {
                    let _ = w.unmaximize();
                } else {
                    let _ = w.maximize();
                }
            }
        }
        "new_window" => {
            let s = app.state::<AppState>().settings.lock().unwrap().clone();
            let n = app.state::<AppState>().next_window.fetch_add(1, Ordering::SeqCst);
            let _ = build_app_window(app, &format!("win-{n}"), &s);
        }
        "clear_cache" => {
            if let Ok(cache) = app.path().app_cache_dir() {
                let _ = std::fs::remove_dir_all(&cache);
            }
            app.restart();
        }
        "always_on_top" => {
            let state = app.state::<AppState>();
            let mut s = state.settings.lock().unwrap().clone();
            s.always_on_top = !s.always_on_top;
            *state.settings.lock().unwrap() = s.clone();
            save_settings(app, &s);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_always_on_top(s.always_on_top);
            }
        }
        "devtools" => {
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
    #[allow(unused_mut)]
    let mut script = format!(
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
    );
    #[cfg(feature = "mcp")]
    {
        script.push('\n');
        script.push_str(MCP_TEST_LISTENER);
    }
    script
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

    // Autonomous-testing socket (opt-in: `--features mcp`); never in release.
    #[cfg(feature = "mcp")]
    {
        builder = builder.plugin(tauri_plugin_mcp::init_with_config(
            tauri_plugin_mcp::PluginConfig::new("carrier".to_string())
                .start_socket_server(true)
                .socket_path("/tmp/tauri-mcp-carrier.sock".into()),
        ));
    }

    builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
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
                    let hide = handle.state::<AppState>().settings.lock().unwrap().hide_on_close;
                    if hide {
                        api.prevent_close();
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                }
            });

            apply_settings(app.handle(), &settings);

            if settings.start_to_tray {
                let _ = window.hide();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Carrier");
}
