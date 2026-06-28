//! Carrier — a tiny, distraction-free desktop client for Facebook Messenger.
//!
//! Opens a WebView window pointed at the Messenger web app, injects a stylesheet
//! that hides Facebook's surrounding chrome, and adds quality-of-life features:
//! shortcuts, zoom, an image viewer, a settings panel, copy/download image,
//! native notifications, theme sync, and tracking-redirect-free external links.
//! Anything that isn't Messenger is handed to the user's default browser.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{AboutMetadata, Menu, MenuItem, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    webview::{Color, DownloadEvent},
    Listener, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;
use url::Url;

/// The page we wrap.
const HOME_URL: &str = "https://www.facebook.com/messages";

/// Window/app title. Debug builds are marked so a dev build (e.g. the
/// tauri-mcp one) isn't mistaken for a release install.
const APP_TITLE: &str = if cfg!(debug_assertions) {
    "Carrier (debug)"
} else {
    "Carrier"
};

/// Injected assets (see `inject/`).
const INJECT_CSS: &str = include_str!("../inject/messenger.css");
const INJECT_JS: &str = include_str!("../inject/messenger.js");
const INJECT_PANEL: &str = include_str!("../inject/panel.js");

// Dev-only (`mcp` feature): the tauri-plugin-mcp guest responder, injected into
// the remote Facebook page so execute_js / get_dom round-trips work. Empty in
// release builds, so the JS-eval responder never ships.
#[cfg(feature = "mcp")]
const INJECT_MCP_BRIDGE: &str = include_str!("../inject/mcp-bridge.js");
#[cfg(not(feature = "mcp"))]
const INJECT_MCP_BRIDGE: &str = "";

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
    /// Show the unread count on the Dock/taskbar icon.
    unread_badge: bool,
    /// What the unread badge counts: "messages" (Facebook's total unread message
    /// count, from the page title) or "conversations" (unread chats in the list).
    badge_mode: String,
    /// Force the Messenger theme: "system" (follow FB), "light", or "dark".
    theme: String,
    /// Hide the conversation-info side panel for a roomier chat view.
    compact: bool,
    /// macOS: run as a menu-bar app with no Dock icon (requires the tray).
    menu_bar_only: bool,
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
            unread_badge: true,
            badge_mode: "messages".into(),
            theme: "system".into(),
            compact: false,
            menu_bar_only: false,
        }
    }
}

struct AppState {
    settings: Mutex<Settings>,
    tray: Mutex<Option<TrayIcon>>,
    next_window: AtomicUsize,
    /// True while [`recreate_themed_windows`] is between destroying and
    /// rebuilding, so the run loop doesn't exit when the window count hits zero.
    recreating: std::sync::atomic::AtomicBool,
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

/// Show the main window if it's hidden/unfocused, or hide it if it's already the
/// focused window — so a tray click toggles the app.
fn toggle_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);
        if visible && focused {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }
}

/// Whether a tray icon should exist: when the user asked for one, or when
/// menu-bar-only mode is on (the only way back to a Dock-less app).
fn wants_tray(s: &Settings) -> bool {
    s.show_tray || s.menu_bar_only
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<TrayIcon> {
    // Left-click toggles the window; right-click offers only Quit (showing is the
    // click itself, so a separate "Open" item would be redundant).
    let quit_item = MenuItem::with_id(app, "quit", "Quit Carrier", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    TrayIconBuilder::with_id("carrier-tray")
        .tooltip(APP_TITLE)
        .icon(app.default_window_icon().expect("bundled icon").clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id.as_ref() == "quit" {
                app.exit(0);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main(tray.app_handle());
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
    let theme = theme_for(s);
    for (label, window) in app.webview_windows() {
        // Apply to every window (incl. the Settings dialog) so toggling Always
        // on Top from the dialog doesn't leave the dialog stuck behind the
        // now-topmost Messenger windows.
        let _ = window.set_always_on_top(s.always_on_top);
        // Webview color-scheme (and the window chrome on Windows/Linux).
        let _ = window.set_theme(theme);
        // The (now-transparent) webview lets the window background show through
        // the title bar, so keep that background in step with the theme. Tauri's
        // own set_background_color is unreliable on macOS (it can invert white to
        // black — tauri#12349), so set the NSWindow colour directly there.
        #[cfg(target_os = "macos")]
        {
            let win = window.clone();
            let dark = is_dark(s);
            let _ = window.run_on_main_thread(move || {
                if let Ok(ptr) = win.ns_window() {
                    set_macos_window_bg(ptr, dark);
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window.set_background_color(Some(splash_background(s)));
        if label != "settings" {
            // Push the new prefs to the running page so JS-side settings
            // (spell-check) refresh without a reload.
            if let Some(ref json) = settings_json {
                let _ = window.eval(format!(
                    "window.__CARRIER_SETTINGS__ = {json}; \
                     try {{ localStorage.setItem('__carrier_settings', JSON.stringify(window.__CARRIER_SETTINGS__)); }} catch (e) {{}} \
                     window.dispatchEvent(new Event('carrier:settings'));"
                ));
            }
        }
    }

    // Tray: create or tear down. Menu-bar-only needs one (it's the only way to
    // reach a Dock-less app), so force it on then.
    let want_tray = wants_tray(s);
    let state = app.state::<AppState>();
    let mut tray = state.tray.lock().unwrap();
    match (want_tray, tray.is_some()) {
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
    // Whether a tray icon is actually present after the reconcile above (e.g.
    // build_tray may have failed). macOS uses this to avoid hiding the Dock with
    // no tray to fall back on.
    #[cfg(target_os = "macos")]
    let tray_available = tray.is_some();
    drop(tray);

    // macOS: hide/show the Dock icon (menu-bar-only mode). Only go Dock-less when
    // a tray exists to reach the app from — otherwise the app would have neither a
    // Dock icon nor a tray and be unreachable, so stay Regular and show the window.
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(if s.menu_bar_only && tray_available {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        });
        if s.menu_bar_only && !tray_available {
            show_main(app);
        }
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

/// Strip path separators and shell-unsafe characters so a page-supplied name
/// can't escape the Downloads folder; falls back to "download".
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

/// True for filenames whose extension is a directly-executable type, so a remote
/// page can't quietly drop malware in Downloads. Media, documents and archives
/// (the things you'd actually save from Messenger) are all allowed.
fn is_unsafe_download(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "exe"
            | "msi"
            | "bat"
            | "cmd"
            | "com"
            | "scr"
            | "ps1"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "dmg"
            | "pkg"
            | "app"
            | "command"
            | "scpt"
            | "sh"
            | "bash"
            | "zsh"
            | "run"
            | "bin"
            | "jar"
            | "jnlp"
            | "deb"
            | "rpm"
            | "appimage"
    )
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
    let prev_theme = state.settings.lock().unwrap().theme.clone();
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
    // macOS needs a window rebuild to re-theme the title bar; other platforms
    // already re-themed the chrome live in apply_settings.
    recreate_on_theme_change(app, &prev_theme, &effective.theme);
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

/// Clear the WebView's cookies/cache/storage, then relaunch.
fn clear_cache(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.clear_all_browsing_data();
    }
    if let Ok(cache) = app.path().app_cache_dir() {
        let _ = std::fs::remove_dir_all(&cache);
    }
}

/// Check GitHub releases for an update; download & install if found.
async fn run_update_check(app: &tauri::AppHandle) -> Result<String, String> {
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

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<String, String> {
    run_update_check(&app).await
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// The window/chrome theme to apply for a given preference: an explicit
/// light/dark, or `None` to follow the system.
fn theme_for(s: &Settings) -> Option<tauri::Theme> {
    match s.theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    }
}

/// True when the window should render dark (forced dark, or system-dark).
fn is_dark(s: &Settings) -> bool {
    match s.theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => matches!(dark_light::detect(), dark_light::Mode::Dark),
    }
}

/// A theme-appropriate window background so there's no white flash before the
/// remote page paints (Facebook glares white in dark mode while loading).
fn splash_background(s: &Settings) -> Color {
    if is_dark(s) {
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
    .title(APP_TITLE)
    .inner_size(1200.0, 780.0)
    .min_inner_size(420.0, 520.0)
    .theme(theme_for(settings))
    .background_color(splash_background(settings))
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
            // Prefer the WebView's suggested filename — it carries the real
            // extension (e.g. a blob the page named via `download="photo.png"`);
            // fall back to the URL's last path segment.
            let suggested = destination
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| filename_from_url(&url));
            let name = sanitize_filename(&suggested);
            // Don't silently save an executable a page might push to Downloads.
            if is_unsafe_download(&name) {
                return false;
            }
            // Fail closed: if we can't resolve/create the Downloads folder we
            // can't enforce where the file lands, so refuse rather than let the
            // WebView write to its own chosen destination.
            let Some(dir) = downloads_dir() else {
                return false;
            };
            if std::fs::create_dir_all(&dir).is_err() {
                return false;
            }
            *destination = unique_path(dir.join(name));
        }
        true
    })
    .build()
    .inspect(|window| {
        // New windows inherit the current always-on-top preference.
        let _ = window.set_always_on_top(settings.always_on_top);
        // macOS: let the themed window background show through the title bar.
        #[cfg(target_os = "macos")]
        make_webview_transparent(window);
    })
}

/// Disable the WKWebView's opaque white background so the window background —
/// which we keep in step with the theme — shows through the title bar and
/// overscroll areas. Tauri leaves the webview background unimplemented on macOS,
/// so flip the private `drawsBackground` flag ourselves (the same thing wry does
/// for transparent windows).
#[cfg(target_os = "macos")]
fn make_webview_transparent(window: &WebviewWindow) {
    let _ = window.with_webview(|webview| {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::NSString;

        let wk = webview.inner() as *mut AnyObject;
        if wk.is_null() {
            return;
        }
        // SAFETY: `wk` is the live WKWebView; -setValue:forKey: with @NO on the
        // private `drawsBackground` key runs on the main thread.
        unsafe {
            let no: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: false];
            let key = NSString::from_str("drawsBackground");
            let _: () = msg_send![wk, setValue: no, forKey: &*key];
        }
    });
}

/// Set the NSWindow background colour directly — Facebook dark, or white — so the
/// transparent webview shows the right colour in the title bar. Tauri's
/// set_background_color is unreliable on macOS, so we message AppKit ourselves.
/// Must run on the main thread.
#[cfg(target_os = "macos")]
fn set_macos_window_bg(ns_window: *mut std::ffi::c_void, dark: bool) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    if ns_window.is_null() {
        return;
    }
    // SAFETY: `ns_window` is this window's live NSWindow*; NSColor factory
    // methods and -setBackgroundColor: run on the main thread.
    unsafe {
        let ns_window = ns_window as *mut AnyObject;
        let color: *mut AnyObject = if dark {
            // Facebook dark, matching splash_background.
            msg_send![class!(NSColor), colorWithSRGBRed: 24.0f64 / 255.0, green: 25.0f64 / 255.0, blue: 26.0f64 / 255.0, alpha: 1.0f64]
        } else {
            msg_send![class!(NSColor), whiteColor]
        };
        let _: () = msg_send![ns_window, setBackgroundColor: color];
    }
}

/// On macOS the window/title-bar theme is fixed at creation, so a live theme
/// change needs a full window rebuild; other platforms re-theme the chrome live
/// in `apply_settings`, so this is a no-op there (rebuilding would needlessly
/// reload Messenger and drop in-progress UI state).
#[cfg(target_os = "macos")]
fn recreate_on_theme_change(app: &tauri::AppHandle, prev: &str, next: &str) {
    if prev != next {
        recreate_themed_windows(app);
    }
}

#[cfg(not(target_os = "macos"))]
fn recreate_on_theme_change(_app: &tauri::AppHandle, _prev: &str, _next: &str) {}

/// Install the `main` window's close behaviour: hide to the tray when
/// `hide_on_close` is set and a tray exists, otherwise quit. Reinstalled on every
/// `main` window the app creates (startup and after a themed rebuild) so the
/// behaviour survives `recreate_themed_windows`.
fn install_main_close_handler(app: &tauri::AppHandle, window: &WebviewWindow) {
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            let (hide, has_tray) = {
                let state = handle.state::<AppState>();
                let hide = state.settings.lock().unwrap().hide_on_close;
                let has_tray = state.tray.lock().unwrap().is_some();
                (hide, has_tray)
            };
            // Only hide to the tray if one was actually created (tray creation can
            // fail, e.g. on a Linux session without an AppIndicator); otherwise
            // closing the main window quits the app (don't let an open Settings
            // dialog keep it running).
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
}

/// Ask macOS for notification authorization, including the **badge** option.
///
/// Since macOS 12 (Monterey), `[[NSApp dockTile] setBadgeLabel:]` — which is what
/// Tauri's `set_badge_count` calls under the hood — is silently ignored unless the
/// app has requested `UNUserNotificationCenter` authorization with the badge
/// option. Carrier's notifications go through `notify-rust` (the legacy
/// `NSUserNotification` path), which never asks, so the unread count the page set
/// via `set_badge_count` produced no Dock badge at all (issue #5). Requesting it
/// makes the badge work; the grant is persisted by the OS, so on later launches
/// this resolves immediately without a prompt.
///
/// Must run on the main thread, and only takes effect once the app has finished
/// launching — calling it from `setup` is a silent no-op — so it's invoked from
/// the `RunEvent::Ready` handler. Safe to call unconditionally: if the user
/// denies notifications the badge simply won't show, exactly as before.
#[cfg(target_os = "macos")]
fn request_badge_authorization() {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    let center = UNUserNotificationCenter::currentNotificationCenter();
    let options = UNAuthorizationOptions::Badge
        | UNAuthorizationOptions::Alert
        | UNAuthorizationOptions::Sound;
    // The completion handler is required by the API; the OS persists the grant,
    // so there's nothing to do with the result. The framework copies the block,
    // so letting our `RcBlock` drop when this returns is fine.
    let handler = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(options, &handler);
}

/// Rebuild every Messenger window (not the Settings dialog) with the current
/// settings. The macOS title bar's theme is fixed at window creation — no
/// runtime call repaints it — so a live theme switch is reflected by recreating
/// the window. Each rebuilt window keeps its place and size; the page reloads
/// (the login session is preserved by the persisted cookies). Runs off the
/// event-loop handler and destroys before rebuilding so the label is free.
#[cfg(target_os = "macos")]
fn recreate_themed_windows(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    // Claim the "recreating" flag synchronously: if a rebuild is already in
    // flight, skip this one. Setting it inside the spawned task would let two
    // rapid theme switches overlap, and the second could clear the flag mid-way
    // through the first's zero-window window — letting the app exit.
    if app
        .state::<AppState>()
        .recreating
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Snapshot label + geometry, then destroy (not close — close would just
        // hide it), so we can rebuild each window where it was.
        let targets: Vec<(String, _)> = app
            .webview_windows()
            .into_iter()
            .filter(|(label, _)| label != "settings")
            .map(|(label, window)| {
                let geometry = window.outer_position().ok().zip(window.inner_size().ok());
                let _ = window.destroy();
                (label, geometry)
            })
            .collect();
        if !targets.is_empty() {
            // Let the event loop finish destroying so the labels are free again.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            // Read settings after the wait so a theme change during it is honoured.
            let settings = app.state::<AppState>().settings.lock().unwrap().clone();
            for (label, geometry) in targets {
                if let Ok(window) = build_app_window(&app, &label, &settings) {
                    // The rebuilt main window must re-acquire the close-to-tray
                    // handler startup installed on the original.
                    if label == "main" {
                        install_main_close_handler(&app, &window);
                    }
                    if let Some((pos, size)) = geometry {
                        let _ = window.set_position(tauri::Position::Physical(pos));
                        let _ = window.set_size(tauri::Size::Physical(size));
                    }
                }
            }
        }
        app.state::<AppState>()
            .recreating
            .store(false, Ordering::SeqCst);
    });
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
    let (aot, theme) = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().unwrap();
        (s.always_on_top, theme_for(&s))
    };
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title(format!("{APP_TITLE} Settings"))
        .inner_size(460.0, 620.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(aot)
        .theme(theme)
        .build();
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
    let app_menu = SubmenuBuilder::new(app, APP_TITLE)
        .about(Some(AboutMetadata::default()))
        .separator()
        .item(&prefs)
        .separator()
        .hide()
        .separator()
        .quit()
        .build()?;

    let new_window = mi("new_window", "New Window", Some("CmdOrCtrl+N"))?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&new_window)
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
    let theme_sys = mi("theme_system", "System", None)?;
    let theme_light = mi("theme_light", "Light", None)?;
    let theme_dark = mi("theme_dark", "Dark", None)?;
    let theme_menu = SubmenuBuilder::new(app, "Theme")
        .item(&theme_sys)
        .item(&theme_light)
        .item(&theme_dark)
        .build()?;
    let compact = mi("compact", "Toggle Compact Mode", Some("CmdOrCtrl+Shift+S"))?;
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
            .item(&theme_menu)
            .item(&compact)
            .item(&aot);
        #[cfg(debug_assertions)]
        let b = b.separator().item(&devtools);
        let _ = &devtools;
        b.build()?
    };

    let maximize = mi("maximize", "Zoom", None)?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .item(&maximize)
        .separator()
        .close_window()
        .build()?;

    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window])
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

/// Apply a settings change made from the native menu: mutate, persist, re-apply.
/// (Used for view-style toggles — not autostart, which syncs separately.)
fn mutate_settings(app: &tauri::AppHandle, f: impl FnOnce(&mut Settings)) {
    let state = app.state::<AppState>();
    // Mutate in place under the lock so concurrent callers can't read-modify-write
    // a stale clone and lose each other's changes. Persist/apply after releasing
    // it (apply_settings touches windows and must not run while holding the lock).
    let (prev_theme, s) = {
        let mut settings = state.settings.lock().unwrap();
        let prev_theme = settings.theme.clone();
        f(&mut settings);
        (prev_theme, settings.clone())
    };
    if let Err(e) = save_settings(app, &s) {
        eprintln!("carrier: failed to save settings: {e}");
    }
    apply_settings(app, &s);
    // macOS needs a window rebuild to re-theme the title bar; other platforms
    // already re-themed the chrome live in apply_settings.
    recreate_on_theme_change(app, &prev_theme, &s.theme);
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
        "zoom_in" => eval("window.__carrierZoomIn && window.__carrierZoomIn()"),
        "zoom_out" => eval("window.__carrierZoomOut && window.__carrierZoomOut()"),
        "zoom_reset" => eval("window.__carrierZoomReset && window.__carrierZoomReset()"),
        "paste_match_style" => eval(
            "navigator.clipboard && navigator.clipboard.readText().then(function (t) { \
             document.execCommand('insertText', false, t); })",
        ),
        "theme_system" => mutate_settings(app, |s| s.theme = "system".into()),
        "theme_light" => mutate_settings(app, |s| s.theme = "light".into()),
        "theme_dark" => mutate_settings(app, |s| s.theme = "dark".into()),
        "compact" => mutate_settings(app, |s| s.compact = !s.compact),
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
        "always_on_top" => mutate_settings(app, |s| s.always_on_top = !s.always_on_top),
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
  // Prefer settings cached in localStorage (written by apply_settings on every
  // change) over this baked-in snapshot, so an in-session settings change
  // survives Facebook reloading the page (which re-runs this script). Falls back
  // to the snapshot on first load / if storage was cleared.
  var baked = {settings_literal};
  try {{
    var stored = JSON.parse(localStorage.getItem('__carrier_settings') || 'null');
    window.__CARRIER_SETTINGS__ = stored && typeof stored === 'object' ? stored : baked;
  }} catch (e) {{
    window.__CARRIER_SETTINGS__ = baked;
  }}
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
{INJECT_PANEL}
{INJECT_MCP_BRIDGE}"#
    )
}

pub fn run() {
    let initial = load_settings_early();

    let mut builder = tauri::Builder::default();

    // Dev-only (the `mcp` feature): expose the webview to tauri-plugin-mcp for
    // DOM/JS inspection. Not in the dependency graph for normal/release builds.
    #[cfg(feature = "mcp")]
    {
        builder = builder.plugin(tauri_plugin_mcp::init_with_config(
            tauri_plugin_mcp::PluginConfig::new(APP_TITLE.to_string())
                .start_socket_server(true)
                .socket_path("/tmp/tauri-mcp.sock".into()),
        ));
    }

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
        .plugin(tauri_plugin_opener::init())
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
            recreating: std::sync::atomic::AtomicBool::new(false),
        })
        .menu(build_menu)
        .on_menu_event(handle_menu_event)
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            reset_settings,
            check_for_updates
        ])
        .setup(move |app| {
            let settings = load_settings(app.handle());
            *app.state::<AppState>().settings.lock().unwrap() = settings.clone();

            let window = build_app_window(app.handle(), "main", &settings)?;

            // Close button: hide to tray (if enabled) instead of quitting.
            // A themed rebuild reinstalls this on the new main window too.
            install_main_close_handler(app.handle(), &window);

            // Don't sync autostart at startup; the OS registration already
            // reflects the user's last explicit choice.
            apply_settings(app.handle(), &settings);

            // Start hidden only when a tray was actually created to reopen from.
            let has_tray = app.state::<AppState>().tray.lock().unwrap().is_some();
            if settings.start_to_tray && has_tray {
                let _ = window.hide();
            }

            // The Facebook page is a remote origin and can't call Carrier's own
            // commands, so the F3/F2 shortcuts emit events that we handle here.
            let h = app.handle().clone();
            app.listen_any("carrier:open-settings", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move { show_settings_window(&h) });
            });
            let h = app.handle().clone();
            app.listen_any("carrier:check-updates", move |_| {
                let h = h.clone();
                tauri::async_runtime::spawn(async move {
                    if let Ok(msg) = run_update_check(&h).await {
                        if msg == "up-to-date" {
                            let _ = h
                                .notification()
                                .builder()
                                .title("Carrier")
                                .body("Carrier is up to date.")
                                .show();
                        }
                    }
                });
            });

            // Unread count from the page → tray tooltip (the Dock badge is set
            // page-side; this keeps the tray useful in menu-bar-only mode).
            let h = app.handle().clone();
            app.listen_any("carrier:unread", move |event| {
                let n: i64 = event.payload().trim().parse().unwrap_or(0);
                if let Some(tray) = h.state::<AppState>().tray.lock().unwrap().as_ref() {
                    let tip = if n > 0 {
                        format!("{APP_TITLE} — {n} unread")
                    } else {
                        APP_TITLE.to_string()
                    };
                    let _ = tray.set_tooltip(Some(&tip));
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Carrier")
        .run(|app, event| {
            // macOS hides the Dock badge unless the app has requested
            // notification authorization with the badge option. Do it once the
            // app is ready (UNUserNotificationCenter needs the app fully
            // launched — calling it during setup is a silent no-op). See
            // `request_badge_authorization` and issue #5.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Ready = event {
                request_badge_authorization();
            }

            // A theme switch destroys and rebuilds the windows; don't let the
            // momentary zero-window state quit the app.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if app
                    .state::<AppState>()
                    .recreating
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    api.prevent_exit();
                }
            }
        });
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

    // -----------------------------------------------------------------------
    // is_unsafe_download  (new in this PR)
    // -----------------------------------------------------------------------

    #[test]
    fn unsafe_download_blocks_all_listed_executable_extensions() {
        // Every extension in the explicit blocklist must be rejected.
        let blocked = [
            "malware.exe",
            "setup.msi",
            "run.bat",
            "run.cmd",
            "trojan.com",
            "screen.scr",
            "evil.ps1",
            "script.vbs",
            "script.vbe",
            "payload.js",
            "payload.jse",
            "config.wsf",
            "config.wsh",
            "app.hta",
            "installer.dmg",
            "package.pkg",
            "bundle.app",
            "run.command",
            "autorun.scpt",
            "start.sh",
            "start.bash",
            "start.zsh",
            "start.run",
            "payload.bin",
            "library.jar",
            "webstart.jnlp",
            "package.deb",
            "package.rpm",
            "portable.appimage",
        ];
        for name in &blocked {
            assert!(
                is_unsafe_download(name),
                "expected {name} to be blocked as unsafe"
            );
        }
    }

    #[test]
    fn unsafe_download_allows_safe_media_and_document_extensions() {
        // Common file types users legitimately save from Messenger must be allowed.
        let allowed = [
            "photo.jpg",
            "image.jpeg",
            "picture.png",
            "animation.gif",
            "photo.webp",
            "clip.mp4",
            "video.mov",
            "video.avi",
            "audio.mp3",
            "audio.wav",
            "audio.ogg",
            "document.pdf",
            "spreadsheet.xlsx",
            "presentation.pptx",
            "archive.zip",
            "archive.tar",
            "archive.gz",
            "archive.7z",
            "text.txt",
            "data.csv",
            "data.json",
        ];
        for name in &allowed {
            assert!(
                !is_unsafe_download(name),
                "expected {name} to be allowed (safe extension)"
            );
        }
    }

    #[test]
    fn unsafe_download_is_case_insensitive() {
        // Extensions should be compared case-insensitively.
        assert!(is_unsafe_download("VIRUS.EXE"));
        assert!(is_unsafe_download("Script.Ps1"));
        assert!(is_unsafe_download("Payload.SH"));
        assert!(!is_unsafe_download("Photo.PNG"));
        assert!(!is_unsafe_download("Clip.MP4"));
    }

    #[test]
    fn unsafe_download_no_extension_is_safe() {
        // A filename with no extension at all is allowed (not executable by extension).
        assert!(!is_unsafe_download("filenoext"));
        assert!(!is_unsafe_download("download"));
    }

    #[test]
    fn unsafe_download_dotfile_edge_cases() {
        // A dotfile whose name is exactly the "extension" portion — rsplit_once('.') returns
        // ("", "sh") for ".sh", so ".sh" is blocked; ".gitignore" has ext "gitignore" (safe).
        assert!(is_unsafe_download(".sh"));
        assert!(!is_unsafe_download(".gitignore"));
        // Multiple dots: only the last segment is checked.
        assert!(is_unsafe_download("setup.tar.exe"));
        assert!(!is_unsafe_download("setup.exe.zip"));
    }

    // -----------------------------------------------------------------------
    // theme_for / is_dark / splash_background  (new in this PR)
    // -----------------------------------------------------------------------

    fn with_theme(theme: &str) -> Settings {
        Settings {
            theme: theme.into(),
            ..Default::default()
        }
    }

    #[test]
    fn theme_for_dark_returns_dark() {
        assert_eq!(theme_for(&with_theme("dark")), Some(tauri::Theme::Dark));
    }

    #[test]
    fn theme_for_light_returns_light() {
        assert_eq!(theme_for(&with_theme("light")), Some(tauri::Theme::Light));
    }

    #[test]
    fn theme_for_system_returns_none() {
        assert_eq!(theme_for(&with_theme("system")), None);
    }

    #[test]
    fn theme_for_unknown_string_returns_none() {
        assert_eq!(theme_for(&with_theme("auto")), None);
        assert_eq!(theme_for(&with_theme("")), None);
    }

    // "system" calls dark_light::detect() (OS-dependent), so only the
    // explicitly-forced cases are asserted here.
    #[test]
    fn is_dark_forced_dark() {
        assert!(is_dark(&with_theme("dark")));
    }

    #[test]
    fn is_dark_forced_light() {
        assert!(!is_dark(&with_theme("light")));
    }

    #[test]
    fn splash_background_dark_is_facebook_dark_color() {
        // Facebook's dark background colour, as hard-coded in the function.
        assert_eq!(
            splash_background(&with_theme("dark")),
            Color(24, 25, 26, 255)
        );
    }

    #[test]
    fn splash_background_light_is_white() {
        assert_eq!(
            splash_background(&with_theme("light")),
            Color(255, 255, 255, 255)
        );
    }

    // -----------------------------------------------------------------------
    // Settings::default  (new fields added in this PR)
    // -----------------------------------------------------------------------

    #[test]
    fn settings_default_new_fields_have_correct_values() {
        let s = Settings::default();
        assert!(s.unread_badge, "unread_badge should default to true");
        assert_eq!(s.theme, "system", "theme should default to 'system'");
        assert!(!s.compact, "compact should default to false");
        assert!(!s.menu_bar_only, "menu_bar_only should default to false");
    }

    // -----------------------------------------------------------------------
    // want_tray logic  (new in this PR: show_tray || menu_bar_only)
    // -----------------------------------------------------------------------

    #[test]
    fn wants_tray_true_when_show_tray_set() {
        // show_tray defaults to true.
        assert!(wants_tray(&Settings::default()));
    }

    #[test]
    fn wants_tray_menu_bar_only_forces_it_even_without_show_tray() {
        let s = Settings {
            show_tray: false,
            menu_bar_only: true,
            ..Default::default()
        };
        assert!(wants_tray(&s), "menu_bar_only must force the tray on");
    }

    #[test]
    fn wants_tray_false_when_both_off() {
        let s = Settings {
            show_tray: false,
            menu_bar_only: false,
            ..Default::default()
        };
        assert!(!wants_tray(&s), "no tray when both are off");
    }

    // -----------------------------------------------------------------------
    // percent_decode  (used by filename_from_url; boundary cases)
    // -----------------------------------------------------------------------

    #[test]
    fn percent_decode_handles_encoded_chars() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("photo%2Fname.jpg"), "photo/name.jpg");
        assert_eq!(percent_decode("%2F%00"), "/\0");
    }

    #[test]
    fn percent_decode_leaves_plain_text_unchanged() {
        assert_eq!(percent_decode("photo.png"), "photo.png");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn percent_decode_incomplete_sequence_is_kept_literally() {
        // A trailing lone '%' or short sequence must not panic.
        assert_eq!(percent_decode("abc%"), "abc%");
        assert_eq!(percent_decode("abc%2"), "abc%2");
    }
}
