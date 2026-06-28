# Carrier — project guide & agent handoff

Carrier is a tiny **Tauri v2** desktop client for **Facebook Messenger** (wraps
`facebook.com/messages` in a chrome-stripped native window). Repo:
**`kristofferR/carrier`** (use the **kristofferR** GitHub account). **v1.0.0 is
released.** **Cross-platform: macOS, Windows, and Linux** (CI builds all six
targets). The "macOS theme rendering" section below is platform-specific; on
Windows/Linux the window chrome follows `set_theme` directly and there's none of
the WKWebView/title-bar trouble.

---

## Build / run / install

```bash
# from repo root
bun install
bun run tauri build               # release-ish bundle (debug symbols unless --release config)
bun run tauri build --debug --bundles app   # fast debug .app only

# signed macOS release (Developer ID) — for the real install:
export APPLE_SIGNING_IDENTITY="Developer ID Application: Kristoffer Risanger (S5Q742QZEL)"
bun run tauri build               # uses src-tauri/tauri.conf.release.json via the CI flow

# install without a Gatekeeper prompt (rm -rf on /Applications is blocked by the safety net):
ditto "<path>/Carrier.app" "/Applications/Carrier.app"
xattr -dr com.apple.quarantine "/Applications/Carrier.app"
```

**Gate before every commit** (CI mirrors this):
```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --lib
node --check inject/messenger.js
```

Releases are tag-driven: push `v*` → `.github/workflows/release.yml` builds 6
targets, signs + notarizes macOS, and auto-publishes (Apple/Tauri secrets are set
in the repo).

---

## tauri-mcp — dev webview inspection (set up this session; activates next session)

Lets an agent inspect/drive the running webview: `execute_js`, `query_page`
(DOM), **`take_screenshot` (background — no window popping to the foreground)**,
click/type, etc. Plugin: [P3GLEG/tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp).

- Gated behind the Cargo **`mcp` feature** → **release builds never compile it**.
- Build with it: `bun run tauri build --debug --features mcp --bundles app`.
  The running app then opens the IPC socket `/tmp/tauri-mcp.sock` and its window
  title reads **"Carrier (debug)"** (all debug builds are marked via `APP_TITLE`).
- `.mcp.json` registers the `tauri-mcp` server (`npx -y tauri-plugin-mcp-server`,
  `TAURI_MCP_IPC_PATH=/tmp/tauri-mcp.sock`). **On a new session you'll be prompted
  to approve the project MCP server — approve it.**
- **Carrier (debug) must be RUNNING** for the tools to connect. A "Carrier (debug)"
  mcp build is currently installed in `/Applications`.

---

## Architecture

- **`src-tauri/src/lib.rs`** — the Rust shell: window/tray/menu/settings/theme,
  `on_navigation` (off-site → default browser), `on_download` (media only, blocks
  executables), updater.
- **`src-tauri/inject/`** — injected at document-start: `messenger.css` (hides FB
  chrome + theme/compact/login CSS), `messenger.js` (shortcuts, zoom, image
  viewer, notifications, unread badge, force-theme, login tidy), `panel.js`
  (toast, settings/update bridge).
- **`dist/settings.html`** — the standalone Settings window.
- **IPC model (important):** the FB page is a **remote origin**, so it **cannot
  call Carrier's own commands**. Page→backend goes through Tauri **plugins**
  (`plugin:opener|open_url`, `plugin:notification|notify`,
  `plugin:window|set_theme`/`set_badge_count`, `plugin:event|emit`) and **core
  events** the Rust side handles via `app.listen_any` (`carrier:open-settings`,
  `carrier:check-updates`, `carrier:unread`). Settings are pushed to the page as
  `window.__CARRIER_SETTINGS__` + a `carrier:settings` event.

---

## macOS theme rendering — hard-won, do NOT re-litigate

The forced light/dark theme (`Settings → Theme`) was a long rabbit hole. Current,
working approach:

- Page theme: `messenger.js` forces FB's `__fb-dark-mode`/`__fb-light-mode` class;
  the palette lives in `messenger.css`.
- **WKWebView is opaque white on macOS** (Tauri leaves the webview bg unimplemented
  there) → it bled through the title bar + login surround. Fixed by flipping the
  private `drawsBackground=NO` via KVC (`make_webview_transparent`).
- **`NSWindow` background set directly via objc** (`set_macos_window_bg`) — Tauri's
  `set_background_color` **inverts white→black on macOS** (tauri#12349).
- **The title bar only themes at WINDOW CREATION.** No runtime call repaints it —
  tried `set_theme`, `NSApplication`/`NSWindow` appearance, `displayIfNeeded`,
  `invalidateShadow`, `setFrame:display:`. So a theme **switch recreates the
  windows** (`recreate_themed_windows`, with a `recreating` flag + `prevent_exit`
  so the brief zero-window state doesn't quit, and a ~150ms delay so the label is
  free). The page reloads; the login session survives via cookies.
- **Login page** ships light-only: `loginTidy` dark-swaps based on the **forced**
  theme (`wantDark`, not the system theme) and clears stray opaque-light wrappers
  **by computed colour** (CSS can't select by colour, and FB's wrappers are
  hash-named) — the palette stays in CSS, JS just finds the stray box.

`objc2`/`objc2-foundation` are macOS-only deps for the above (same versions Tauri
already pulls in).

---

## Current state / outstanding work

- **Theme/login/title-bar fixes:** DONE — committed `4a308e0` on branch
  **`fix/v1.0-review`**.
- **PR #4** (`fix/v1.0-review`) is the **v1.0 code review** PR. Its base was
  re-pointed to **`pre-v1.0`** so CodeRabbit reviews the **entire v1.0 diff**
  (not just the post-release tweaks). **Codex is rate-limited** (can't review).
  CodeRabbit re-flagged the `want_tray` test as tautological — **stale**:
  `wants_tray()` was extracted and the tests call it (verify, then resolve the
  thread). When the loop converges: **re-point PR #4 base → `main` and merge.**
- **Uncommitted right now** (fold into the badge-fix commit): the tauri-mcp setup
  (`Cargo.toml` `mcp` feature + git dep, `lib.rs` plugin reg), `APP_TITLE`
  "(debug)" marking, and `.mcp.json`.
- **Badge bug — issue #5** (1 unread, no Dock badge). `unreadBadge` parses
  `document.title` for `(N)` → `set_badge_count`. Command + permission verified
  correct; likely the title doesn't carry `(N)` (or only when backgrounded).
  **First task next session: launch "Carrier (debug)", use tauri-mcp `execute_js`
  to read `document.title` + the unread DOM, then fix** (maybe DOM-based detection).
- **Cleanup:** reinstall the **clean signed release** over the "Carrier (debug)"
  build for daily use once debugging is done.

---

## Conventions

- GitHub: **kristofferR** (`gh auth switch -u kristofferR`). Trigger CodeRabbit
  **only** through `crq` (never post `@coderabbitai review` directly). A
  `crq autoreview` daemon may be running.
- Commits: branch off (never commit to `main` directly), one logical change,
  end with the `Claude-Session:` footer, **no AI attribution**, non-closing issue
  refs (`Ref #5`).
