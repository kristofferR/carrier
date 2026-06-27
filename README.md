<div align="center">
  <img src="app-icon.png" width="120" alt="Carrier icon" />
  <h1>Carrier</h1>
  <p><strong>A tiny, distraction-free desktop client for Facebook Messenger.</strong></p>
  <p>No Feed, no Reels, no Marketplace — just your conversations, in a native window.</p>
</div>

---

Meta discontinued the official Messenger desktop app, and the standalone
`messenger.com` site was shut down in April 2026. Carrier fills the gap: it wraps
the Messenger web app (`facebook.com/messages`) in a small native window and
strips away the surrounding Facebook chrome.

Built with [Tauri](https://tauri.app) (Rust + the OS's native WebView), so it's a
few MB and uses a fraction of the RAM of an Electron app.

## Features

- **Distraction-free** — a stylesheet hides Facebook's banner, global search,
  Feed/Marketplace/Reels navigation, and tidies the active-chat highlight.
- **Lightweight & native** — one WebView window, no bundled Chromium.
- **Native notifications** — new messages raise real OS notifications even when
  Carrier is in the background.
- **Stays out of the way** — closing hides to the system tray; the app keeps
  running so you still get messages.
- **Settings panel** (<kbd>F3</kbd>) — Always on Top, system tray, Start to Tray,
  Start on System Startup, Hide on Close, spell-check, experimental multi-window.
- **Links open in your browser** — anything that isn't Messenger opens in your
  real default browser (Facebook's `l.php` tracking redirects are stripped
  first). Google/Apple/Microsoft/GitHub logins still work in-app.
- **Auto-updates** — verified updates via the Tauri updater (<kbd>F2</kbd> to
  check now).
- **Full native menu** — back/forward/home, reload, zoom, print, new window,
  clear cache, copy current URL, plus standard edit/window menus.
- **Keyboard shortcuts** — <kbd>F2</kbd> updates, <kbd>F3</kbd> settings,
  <kbd>F5</kbd>/<kbd>Cmd-R</kbd> reload, <kbd>Cmd</kbd>+`[`/`]` back/forward,
  <kbd>Cmd</kbd>+`-`/`=`/`0` zoom.
- **Right-click menus** — copy/download/open images & videos, copy/open links.
  Copying works even for images the page only renders as blobs.
- **Image & video viewer** — double-click a photo or video to open a zoom/pan
  overlay (wheel to zoom, drag or arrow keys to pan, `Esc` to close).
- **Theme-aware** — the native window follows the page's light/dark theme.
- **Calls work** — camera/microphone are requested for Messenger voice & video;
  if the OS has them blocked, Carrier offers to open privacy settings.
- **Remembers its window** — size and position persist between launches.

## Install

Grab the latest installer for your platform from the
[Releases](https://github.com/kristofferR/carrier/releases) page:

| Platform | File |
|----------|------|
| macOS    | `Carrier_*.dmg` |
| Windows  | `Carrier_*-setup.exe` |
| Linux    | `Carrier_*.AppImage` / `*.deb` |

> macOS: the app is ad-hoc signed. On first launch, right-click → **Open** (or
> run `xattr -dr com.apple.quarantine /Applications/Carrier.app`).

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

Carrier is deliberately simple. The Rust shell (`src-tauri/src/lib.rs`):

1. Opens one WebView window at `https://www.facebook.com/messages` with a modern
   browser user-agent (so Facebook serves the full web app).
2. Injects, at document start:
   - [`inject/messenger.css`](src-tauri/inject/messenger.css) — hides the
     Facebook chrome (carefully keeping the media-viewer controls).
   - [`inject/messenger.js`](src-tauri/inject/messenger.js) — keyboard
     shortcuts, page zoom, the image/video viewer, and a fullscreen polyfill.
3. Routes off-site navigation to the default browser, and hides to the tray on
   close.

That's the whole app. To wrap a *different* site, change `HOME_URL`, the
internal-domain list, and the injected CSS.

## Auto-updates

The release workflow is set up to produce Tauri updater artifacts, but
auto-update is **disabled by default** because it needs a signing keypair. To
enable it:

1. `bun run tauri signer generate` → add the private key + password as the
   `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo
   secrets.
2. Add the `updater` plugin to `Cargo.toml` and `lib.rs`, and put the public key
   + endpoint in `tauri.conf.json`. See the
   [Tauri updater docs](https://v2.tauri.app/plugin/updater/).

## Acknowledgements & prior art

Carrier is a clean-room project inspired by the same idea as
[Pake](https://github.com/tw93/Pake) (MIT) — "turn any webpage into a desktop
app with Rust." It exists as an open alternative to the closed-source
`messenger-next` app.

## Disclaimer

Carrier is an unofficial, independent project. It is not affiliated with,
endorsed by, or sponsored by Meta Platforms, Inc. "Facebook" and "Messenger" are
trademarks of their respective owners. Carrier does not modify Facebook's servers
or data; it only restyles the page locally in your own window.

## License

[MIT](LICENSE) © 2026 kristofferR
