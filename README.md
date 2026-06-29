<p align="center">
  <img src="app-icon.png" width="128" height="128" alt="Carrier icon">
</p>

<h1 align="center">Carrier</h1>

<p align="center">
  A tiny, distraction-free desktop client for Facebook Messenger.<br>
  Built with Tauri v2 — runs on macOS, Windows, and Linux.
</p>

<!-- Add docs/screenshot.png (a view you're happy to share publicly) and uncomment:
<p align="center">
  <img src="docs/screenshot.png" alt="Carrier screenshot" width="720">
</p>
-->

<h3 align="center">Download</h3>

<table align="center">
  <tr>
    <td align="center" width="220">
      <a href="https://github.com/kristofferR/carrier/releases/download/v1.0.0/Carrier_1.0.0_mac_arm.dmg">
        <img src="docs/icons/download.svg" width="56" height="56" alt=""><br>
        <strong>Download for macOS</strong><br>
        <sub>Apple Silicon &middot; .dmg</sub>
      </a>
    </td>
    <td align="center" width="220">
      <a href="https://github.com/kristofferR/carrier/releases/download/v1.0.0/Carrier_1.0.0_win_x64_setup.exe">
        <img src="docs/icons/download.svg" width="56" height="56" alt=""><br>
        <strong>Download for Windows</strong><br>
        <sub>64-bit installer &middot; .exe</sub>
      </a>
    </td>
    <td align="center" width="220">
      <a href="https://github.com/kristofferR/carrier/releases/download/v1.0.0/Carrier_1.0.0_lin_x64.AppImage">
        <img src="docs/icons/download.svg" width="56" height="56" alt=""><br>
        <strong>Download for Linux</strong><br>
        <sub>AppImage &middot; x64</sub>
      </a>
    </td>
  </tr>
</table>

<p align="center">
  <sub>
    Intel Mac, Windows ARM, Linux ARM, <code>.deb</code>/<code>.rpm</code>?
    <a href="https://github.com/kristofferR/carrier/releases/latest">See all files →</a>
  </sub>
</p>

---

Meta discontinued the official Messenger desktop app, and the standalone
`messenger.com` site was shut down in April 2026. Carrier fills the gap: it wraps
the Messenger web app (`facebook.com/messages`) in a small native window and
strips away the surrounding Facebook chrome.

Built with [Tauri](https://tauri.app) (Rust + the OS's native WebView) instead of a
bundled Chromium, so the download is tiny — the macOS and Windows installers are
**under 3 MB** (vs. 100 MB+ for an Electron app) — and it idles on a fraction of an
Electron app's RAM. The macOS build is Developer-ID signed and notarized.

## Features

- **Distraction-free** — a stylesheet hides Facebook's banner, global search, and
  Feed/Marketplace/Reels navigation, leaving just your conversations.
- **Lightweight & native** — one WebView window, no bundled Chromium.
- **Native notifications** — new messages raise real OS notifications even when
  Carrier is in the background.
- **Unread badge** — the unread count appears on the Dock / taskbar icon.
- **Force light / dark theme** — keep Messenger (and the window chrome, including
  the macOS title bar) light or dark regardless of the system theme, or follow
  the system.
- **Jump to a conversation** — <kbd>Cmd/Ctrl</kbd>+<kbd>1</kbd>–<kbd>9</kbd> jumps
  to the Nth chat in the list.
- **Toggle conversation info** — show or hide Messenger's conversation-info
  sidebar (<kbd>Cmd/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>I</kbd>).
- **Menu-bar mode** (macOS) — optionally hide the Dock icon and live in the menu
  bar; click the tray icon to toggle the window.
- **Stays out of the way** — closing hides to the tray, so you keep getting
  messages; a tray click brings it back (and hides it again).
- **Links open in your browser** — anything that isn't Messenger opens in your
  real default browser (Facebook's `l.php` tracking redirects are stripped
  first). Google/Apple/Microsoft logins still work in-app.
- **Auto-updates** — verified, signed updates via the Tauri updater
  (<kbd>F2</kbd>, or **Settings → Check for updates**).
- **Right-click menus** — copy/download/open images & videos, copy/open links.
  Copying works even for images the page only renders as blobs.
- **Image & video viewer** — double-click a photo or video for a zoom/pan overlay
  (wheel to zoom, drag or arrow keys to pan, <kbd>Esc</kbd> to close).
- **Calls work** — camera/microphone are requested for Messenger voice & video.
- **Remembers its window** — size and position persist between launches.
- **Settings window** (<kbd>F3</kbd>) — theme, unread badge, hide names &
  avatars, menu-bar mode, always-on-top, tray, start-to-tray, start on login,
  hide-on-close, spell-check, and experimental multi-window.

## Keyboard shortcuts

- <kbd>Cmd/Ctrl</kbd>+<kbd>1</kbd>–<kbd>9</kbd> — jump to the Nth conversation
- <kbd>Cmd/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>I</kbd> — toggle conversation information
- <kbd>Cmd/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> — hide names &amp; avatars
- <kbd>F2</kbd> check for updates &middot; <kbd>F3</kbd> settings &middot;
  <kbd>F5</kbd>/<kbd>Cmd-R</kbd> reload
- <kbd>Cmd</kbd>+<kbd>-</kbd>/<kbd>=</kbd>/<kbd>0</kbd> — zoom out / in / reset

## Install

Grab the installer for your platform from the **Download** box above or the
[Releases](https://github.com/kristofferR/carrier/releases) page. The macOS build
is signed and notarized, so it opens normally — no right-click-Open needed.

## Build from source

Requires [Rust](https://rustup.rs), [Bun](https://bun.sh), and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
git clone https://github.com/kristofferR/carrier.git
cd carrier
bun install
bun run dev      # run in development
bun run build    # produce installers in src-tauri/target/release/bundle/
```

## How it works

The Rust shell (`src-tauri/src/lib.rs`) opens one WebView window at
`https://www.facebook.com/messages` with a modern browser user-agent, then injects
at document start:

- [`inject/messenger.css`](src-tauri/inject/messenger.css) — hides the Facebook
  chrome (carefully keeping the media-viewer controls).
- [`inject/messenger.js`](src-tauri/inject/messenger.js) — keyboard shortcuts,
  page zoom, the image/video viewer, notifications, the unread badge, theme
  forcing, and the adaptive context menu.

Because Facebook is a remote origin, page features reach the backend through Tauri
plugins (opener, notification) and core events rather than custom commands. Off-site
navigation is routed to the default browser, and the window hides to the tray on
close.

## Auto-updates

Carrier ships with the Tauri updater wired up: it checks
`releases/latest/download/latest.json` and can download & install a verified
(minisign-signed) update — press <kbd>F2</kbd> or use **Settings → Check for
updates**.

## Comparison

By using the system's native WebView through Tauri instead of bundling a full
browser engine, Carrier stays small and light on system resources:

| | Carrier | Official Messenger Desktop | Caprine |
| :--- | :---: | :---: | :---: |
| **Status** | ✅ Active | ❌ Discontinued | ⚠️ Unmaintained |
| **Engine** | System WebView (Tauri) | Electron | Electron |
| **Installer size** | ~3 MB | ~100+ MB | ~100 MB |
| **CPU / RAM usage** | Low | High | High |
| **Interface** | Chat only, FB chrome stripped | Chat only | Custom UI |

<sub>The Linux <code>.deb</code>/<code>.rpm</code> are ~4 MB; the self-contained
<code>.AppImage</code> is larger only because it bundles WebKitGTK that the
<code>.deb</code>/<code>.rpm</code> take from the system.</sub>

## Disclaimer

Carrier is an unofficial, independent project. It is not affiliated with, endorsed
by, or sponsored by Meta Platforms, Inc. "Facebook" and "Messenger" are trademarks
of their respective owners. Carrier does not modify Facebook's servers or data; it
only restyles the page locally in your own window.

## License

[MIT](LICENSE) © 2026 kristofferR
