//! Carrier — a tiny, distraction-free desktop client for Facebook Messenger.
//!
//! It opens a single WebView window pointed at the Messenger web app, injects a
//! stylesheet that hides Facebook's surrounding chrome, and adds a few quality
//! of life enhancements (keyboard shortcuts, zoom, an image viewer). Everything
//! else — links to other sites, popups — is handed off to the user's browser.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use url::Url;

/// The page we wrap.
const HOME_URL: &str = "https://www.facebook.com/messages";

/// Injected assets (clean-room; see `inject/`).
const INJECT_CSS: &str = include_str!("../inject/messenger.css");
const INJECT_JS: &str = include_str!("../inject/messenger.js");

/// A modern browser UA so Facebook serves the full Messenger web app rather than
/// a degraded/unsupported-browser experience.
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

/// Domains we keep *inside* the app (Messenger plus the Facebook/Meta auth and
/// media surfaces needed to log in and load content). Anything else is treated
/// as an external link and opened in the user's default browser.
fn is_internal(url: &Url) -> bool {
    match url.scheme() {
        // In-page schemes always stay internal.
        "about" | "blob" | "data" | "javascript" => return true,
        "http" | "https" => {}
        _ => return false,
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

/// Build the WebView initialization script: inject the stylesheet as early as
/// possible (head may not exist yet), then run the enhancement script.
fn init_script() -> String {
    let css_literal = serde_json::to_string(INJECT_CSS).expect("CSS serialises");
    format!(
        r#"(function () {{
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
{INJECT_JS}"#
    )
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let init = init_script();

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(HOME_URL.parse().expect("valid home URL")),
            )
            .title("Carrier")
            .inner_size(1200.0, 780.0)
            .min_inner_size(420.0, 520.0)
            .user_agent(user_agent())
            .initialization_script(&init)
            .on_navigation(|url| {
                // Allow internal navigation; hand external URLs to the browser.
                if is_internal(url) {
                    true
                } else {
                    let _ = open::that(url.as_str());
                    false
                }
            })
            .build()?;

            // Close button hides to tray instead of quitting.
            let app_handle = app.handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Some(w) = app_handle.get_webview_window("main") {
                        let _ = w.hide();
                    }
                }
            });

            // System tray with show / quit.
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
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Carrier");
}
