# Carrier — project guide & agent handoff

Carrier is a tiny **Tauri v2** desktop client for **Facebook Messenger** (wraps
`facebook.com/messages` in a chrome-stripped native window). Repo:
**`kristofferR/Carrier`** (use the **kristofferR** GitHub account). **v1.0.0 is
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

## tauri-mcp — dev webview inspection

Lets an agent inspect/drive the running webview: `execute_js`, `query_page`
(DOM), **`take_screenshot` (background — no window popping to the foreground)**,
click/type, etc. Plugin: [P3GLEG/tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp).
On `main` (committed `d8a25a6`).

- Gated behind the Cargo **`mcp` feature** → **release builds never compile it**.
- Build with it: `bun run tauri build --debug --features mcp --bundles app`. Debug
  builds are marked — the window title reads **"Carrier (debug)"**.
- `.mcp.json` registers the `tauri-mcp` server — **approve the project MCP server
  when a new session prompts for it.**
- **A "Carrier (debug)" build must be RUNNING** for the tools to connect — build it
  with the command above, then `ditto` it into `/Applications`.

---

## Architecture

- **`src-tauri/src/lib.rs`** — the Rust shell: window/tray/menu/settings/theme,
  `on_navigation` (off-site → default browser), `on_download` (media only, blocks
  executables), updater.
- **`src-tauri/inject/`** — injected at document-start: `messenger.css` (hides FB
  chrome + theme/compact/login CSS), `messenger.js` (shortcuts, zoom, image
  viewer, notifications incl. mute / hide-preview privacy, unread badge,
  force-theme, hide names & avatars, login tidy), `panel.js` (toast,
  settings/update bridge).
- **`dist/settings.html`** — the standalone Settings window.
- **IPC model (important):** the FB page is a **remote origin**, so it **cannot
  call Carrier's own commands**. Page→backend goes through Tauri **plugins**
  (`plugin:opener|open_url`,
  `plugin:window|set_theme`/`set_badge_count`, `plugin:event|emit`) and **core
  events** the Rust side handles via `app.listen_any` (`carrier:open-settings`,
  `carrier:check-updates`, `carrier:unread`, and `carrier:notify` — the
  new-message notification bridge, emitted via `plugin:event|emit` and rendered
  natively). Settings are pushed to the page as `window.__CARRIER_SETTINGS__` +
  a `carrier:settings` event.

---

## macOS theme rendering — hard-won, do NOT re-litigate

The forced light/dark theme (`Settings → Theme`) was a long rabbit hole on macOS.
These traps are already worked around in code — don't redo them:

- Tauri's `set_background_color` **inverts white→black on macOS** (tauri#12349), so
  the window background is set directly via objc.
- WKWebView is **opaque white** on macOS and bleeds through the title bar / login
  surround — it's forced transparent via a private API.
- The **title bar only themes at window creation**, so a theme switch **recreates
  the windows** (the page reloads; the login session survives via cookies).
- The login page is light-only; its dark mode is applied by JS off the forced theme.

---

## Current state

v1.0.0 is released; `main` is the trunk. **Live work lives on GitHub, not here** —
`gh issue list` / `gh pr list` for open bugs, enhancements, and WIP branches.
`pre-v1.0` is a kept snapshot base.

---

## Conventions

- GitHub: **kristofferR** (`gh auth switch -u kristofferR`). Trigger CodeRabbit
  **only** through `crq` (never post `@coderabbitai review` directly). A
  `crq autoreview` daemon may be running.
- Commits: branch off by default — though the maintainer may explicitly ask for a
  direct push to `main` (as with the post-v1.0 merge above). One logical change,
  end with the `Claude-Session:` footer, **no AI attribution**, non-closing issue
  refs (`Ref #5`).
