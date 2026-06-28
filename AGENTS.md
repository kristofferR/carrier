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
- Build with it: `bun run tauri build --debug --features mcp --bundles app`.
  The running app then opens the IPC socket `/tmp/tauri-mcp.sock` and its window
  title reads **"Carrier (debug)"** (all debug builds are marked via `APP_TITLE`).
- `.mcp.json` registers the `tauri-mcp` server (`npx -y tauri-plugin-mcp-server`,
  `TAURI_MCP_IPC_PATH=/tmp/tauri-mcp.sock`). **Approve the project MCP server when
  a new session prompts for it.**
- **A "Carrier (debug)" build must be RUNNING** for the tools to connect — build it
  with the command above, then `ditto` it into `/Applications`.

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

- **All post-v1.0 work is merged to `main`** (2026-06-28, tip `e7c9ea2`): the v1.0
  helper tests, `wants_tray()` extraction, the macOS theme/login rendering, the
  tauri-mcp dev tooling, and the CodeRabbit fixes from the #4/#6 reviews (the
  single-flight recreate guard, login-bg restore, macOS-gated rebuild, and the
  close-to-tray handler reattach). It landed by cherry-pick **directly onto `main`**
  over PR #8's CI work — no review PR was merged.
- **Review PRs are done / superseded:** #4 (`fix/v1.0-review`) and #6
  (`review/v1.0-clean`) are closed; #7 (`v1.0-review`) is an **isolated,
  do-not-merge** CodeRabbit review of the squashed v1.0.0 release. Codex stayed
  rate-limited throughout. The dead branches were deleted; `pre-v1.0` is kept as a
  snapshot base.
- **Badge bug — issue #5** (1 unread, no Dock badge) — **still open, in progress on
  branch `fix/badge-issue-5`** (its own worktree). `unreadBadge` parses
  `document.title` for `(N)` → `set_badge_count`; command + permission verified
  correct, so the title likely doesn't carry `(N)` (or only when backgrounded).
  Approach: run a "Carrier (debug)" build, use tauri-mcp `execute_js` to read
  `document.title` + the unread DOM, then fix (likely DOM-based detection).
- **Cleanup:** reinstall the **clean signed release** over any "Carrier (debug)"
  build for daily use once badge debugging is done.

---

## Conventions

- GitHub: **kristofferR** (`gh auth switch -u kristofferR`). Trigger CodeRabbit
  **only** through `crq` (never post `@coderabbitai review` directly). A
  `crq autoreview` daemon may be running.
- Commits: branch off by default — though the maintainer may explicitly ask for a
  direct push to `main` (as with the post-v1.0 merge above). One logical change,
  end with the `Claude-Session:` footer, **no AI attribution**, non-closing issue
  refs (`Ref #5`).
