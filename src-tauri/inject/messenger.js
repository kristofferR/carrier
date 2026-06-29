/*
 * Carrier — in-page enhancements for the Messenger web app.
 * Clean-room implementation (keyboard shortcuts, page zoom, an image/video
 * zoom + pan viewer, and a fullscreen polyfill for the WebView).
 *
 * Runs as a WebView initialization script at document start.
 */
(function () {
  "use strict";

  // Tauri injects initialization scripts into subframes too (notably on
  // Windows). Only enhance the top-level Messenger document.
  if (window.top !== window.self) return;

  /* ----------------------------- Page zoom ------------------------------ */
  const ZOOM_KEY = "carrier:zoom";
  const isWindows = /windows/i.test(navigator.userAgent);

  function applyZoom(percent) {
    const clamped = Math.min(200, Math.max(30, percent));
    if (isWindows) {
      // WebView2 ignores `zoom`; fall back to a transform.
      const scale = clamped / 100;
      document.body.style.transformOrigin = "top left";
      document.body.style.transform = `scale(${scale})`;
      document.body.style.width = `${100 / scale}%`;
      document.body.style.height = `${100 / scale}%`;
    } else {
      document.documentElement.style.zoom = `${clamped}%`;
      window.dispatchEvent(new Event("resize"));
    }
    localStorage.setItem(ZOOM_KEY, String(clamped));
  }

  const currentZoom = () => parseInt(localStorage.getItem(ZOOM_KEY) || "100", 10);
  const zoomIn = () => applyZoom(currentZoom() + 10);
  const zoomOut = () => applyZoom(currentZoom() - 10);
  const zoomReset = () => applyZoom(100);

  /* --------------------------- Keyboard shortcuts ----------------------- */
  // Triggered with the platform accelerator (Cmd on macOS, Ctrl elsewhere).
  const isMac = /mac/i.test(navigator.platform) || /mac/i.test(navigator.userAgent);
  const accel = (e) => (isMac ? e.metaKey : e.ctrlKey);

  const shortcuts = {
    "[": () => history.back(),
    "]": () => history.forward(),
    "-": zoomOut,
    "=": zoomIn,
    "+": zoomIn,
    "0": zoomReset,
    r: () => location.reload(),
  };

  document.addEventListener(
    "keydown",
    (e) => {
      if (!accel(e)) return;
      const fn = shortcuts[e.key];
      if (fn) {
        e.preventDefault();
        fn();
      }
    },
    true,
  );

  /* ------------------------- Tauri bridge + toast ----------------------- */
  // Use the always-present internal bridge directly instead of the global
  // `window.__TAURI__` (which `withGlobalTauri` would also expose to Facebook's
  // own scripts).
  const invoke = (cmd, args) => window.__TAURI_INTERNALS__?.invoke(cmd, args);
  const toast = (msg) =>
    window.__carrierToast ? window.__carrierToast(msg) : console.log("[carrier]", msg);

  /* ------------------------ Plugin command bridges ---------------------- */
  // Facebook is a *remote* origin: Tauri v2 lets it call plugin commands (gated
  // by the capability ACL) but NOT the app's own custom commands. So page
  // features route through plugins, matching how the upstream app works.
  const openUrl = (url) =>
    invoke("plugin:opener|open_url", { url, with: null })?.catch?.(() => {});

  // Expose zoom controls so the native menu (View ▸ Zoom) can drive them.
  window.__carrierZoomIn = zoomIn;
  window.__carrierZoomOut = zoomOut;
  window.__carrierZoomReset = zoomReset;

  /* ----------------------- Function-key shortcuts ----------------------- */
  // F2 check for updates · F3 settings · F5 reload (parity with messenger-next).
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "F5") {
        e.preventDefault();
        location.reload();
      } else if (e.key === "F3") {
        e.preventDefault();
        window.__carrierToggleSettings?.();
      } else if (e.key === "F2") {
        e.preventDefault();
        window.__carrierCheckUpdates?.();
      } else if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && /^[1-9]$/.test(e.key)) {
        // Cmd/Ctrl+1–9: jump to the Nth conversation in the list.
        const target = chatRows()[Number(e.key) - 1];
        if (target) {
          e.preventDefault();
          target.click();
        }
      }
    },
    true,
  );

  // Visible conversation links in the left chat list, in list order.
  function chatRows() {
    const seen = new Set();
    const out = [];
    for (const a of document.querySelectorAll('[role="grid"] a[href*="/t/"], [role="navigation"] a[href*="/t/"]')) {
      const href = a.getAttribute("href");
      if (!href || seen.has(href)) continue;
      const r = a.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue; // skip hidden
      seen.add(href);
      out.push(a);
    }
    return out;
  }

  /* --------------------------- Link handling ---------------------------- */
  // External links open in the real browser (Facebook's l.php tracking
  // redirect is unwrapped on the Rust side). Internal links that would spawn a
  // new window via Shift/Ctrl/Cmd/middle-click navigate in place instead
  // (fixes the "Shift+Click internal links" bug).
  const INTERNAL = ["facebook.com", "messenger.com", "fbcdn.net", "fbsbx.com", "meta.com", "oculus.com"];
  // Facebook's "continue with Google/Apple/Microsoft" social logins use these
  // dedicated auth hosts; keep them in-app so the popup flow works.
  const AUTH_HOSTS = ["accounts.google.com", "login.microsoftonline.com", "appleid.apple.com"];
  function isAuth(u) {
    const host = u.hostname.toLowerCase();
    return AUTH_HOSTS.some((h) => host === h || host.endsWith("." + h));
  }
  function classify(href) {
    try {
      const u = new URL(href, location.href);
      // mailto:/tel: links open in the OS handler.
      if (u.protocol === "mailto:" || u.protocol === "tel:") return { external: true };
      if (!/^https?:$/.test(u.protocol)) return { external: false };
      // Keep OAuth/login popups inside the app so social logins work.
      if (isAuth(u)) return { external: false };
      const host = u.hostname.replace(/^www\./, "");
      const tracking =
        host === "l.facebook.com" ||
        host === "lm.facebook.com" ||
        (host === "facebook.com" && u.pathname === "/l.php");
      const internal = INTERNAL.some((s) => host === s || host.endsWith("." + s));
      return { external: tracking || !internal };
    } catch {
      return { external: false };
    }
  }
  function handleLink(e) {
    const a = e.target.closest?.("a[href]");
    if (!a) return;
    const href = a.href;
    if (!href || href.startsWith("javascript:")) return;
    const modified = e.shiftKey || e.metaKey || e.ctrlKey || e.button === 1;
    const blank = a.target === "_blank";
    if (classify(href).external) {
      e.preventDefault();
      e.stopImmediatePropagation();
      openUrl(href);
    } else if (modified || blank) {
      e.preventDefault();
      e.stopImmediatePropagation();
      location.href = href;
    }
  }
  document.addEventListener("click", handleLink, true);
  document.addEventListener("auxclick", (e) => e.button === 1 && handleLink(e), true);

  /* --------------------- Adaptive context menu -------------------------- */
  // Right-click an image, video or link to get the relevant actions
  // (download / copy / copy address / open in browser), matching the original.
  const filenameFromUrl = (u) => {
    try {
      const p = new URL(u, location.href).pathname.split("/").pop();
      return p && p.includes(".") ? decodeURIComponent(p) : "";
    } catch {
      return "";
    }
  };

  // Download a media src by letting the WebView initiate the download, which the
  // Rust `on_download` handler then writes to Downloads. (Custom commands can't
  // be called from the remote Facebook origin, only plugins / WebView hooks.)
  const MAX_BLOB = 512 * 1024 * 1024;
  async function downloadSrc(src, fallbackName) {
    // Fetch into a same-origin blob so the `download` attribute is honoured (it's
    // ignored for cross-origin URLs) and so we can derive the real extension.
    const res = await fetch(src);
    if (!res.ok) throw new Error(`download failed (${res.status})`);
    const blob = await res.blob();
    if (blob.size > MAX_BLOB) throw new Error("file too large");
    const href = URL.createObjectURL(blob);
    let name = filenameFromUrl(src) || fallbackName;
    if (!name.includes(".")) {
      const ext = ((blob.type || "").split("/")[1] || "").split(";")[0];
      if (ext) name += "." + ext;
    }
    const a = document.createElement("a");
    a.href = href;
    a.download = name;
    a.style.display = "none";
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(href), 10000);
  }

  async function copyImageSrc(src) {
    const res = await fetch(src);
    if (!res.ok) throw new Error(`fetch failed (${res.status})`);
    const blob = await res.blob();
    if (blob.size > MAX_BLOB) throw new Error("image too large");
    await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
  }

  let ctxMenu = null;
  const closeMenu = () => {
    ctxMenu?.remove();
    ctxMenu = null;
    document.removeEventListener("click", closeMenu, true);
    document.removeEventListener("scroll", closeMenu, true);
  };
  document.addEventListener(
    "contextmenu",
    (e) => {
      const t = e.target;
      const video = t.closest?.("video") || t.closest?.("div")?.querySelector?.("video");
      const img = t.closest?.("img[alt]");
      const anchor = t.closest?.("a[href]");
      const imgSrc = img && (img.currentSrc || img.src);
      const vidSrc = video && (video.currentSrc || video.src);
      const linkHref = anchor && anchor.href;

      const items = [];
      if (imgSrc) {
        items.push(["Copy image", () => copyImageSrc(imgSrc).then(() => toast("Image copied")).catch(() => toast("Copy failed"))]);
        items.push(["Download image", () => downloadSrc(imgSrc, "image").then(() => toast("Saved to Downloads")).catch(() => toast("Download failed"))]);
        items.push(["Copy image address", () => navigator.clipboard?.writeText(imgSrc).then(() => toast("Address copied"))]);
        items.push(["Open image in browser", () => openUrl(imgSrc)]);
      } else if (vidSrc) {
        items.push(["Download video", () => downloadSrc(vidSrc, "video").then(() => toast("Saved to Downloads")).catch(() => toast("Download failed"))]);
        items.push(["Copy video address", () => navigator.clipboard?.writeText(vidSrc).then(() => toast("Address copied"))]);
      } else if (linkHref && !linkHref.startsWith("javascript:")) {
        items.push(["Copy link address", () => navigator.clipboard?.writeText(linkHref).then(() => toast("Address copied"))]);
        items.push(["Open link in browser", () => openUrl(linkHref)]);
      }
      if (!items.length) return; // fall through to the native menu (text etc.)

      e.preventDefault();
      closeMenu();
      ctxMenu = document.createElement("div");
      Object.assign(ctxMenu.style, {
        position: "fixed", left: e.clientX + "px", top: e.clientY + "px",
        zIndex: 2147483647, background: "#242526", color: "#e4e6eb",
        border: "1px solid #3a3b3c", borderRadius: "8px", padding: "4px",
        boxShadow: "0 6px 24px rgba(0,0,0,.4)", minWidth: "170px",
        font: "13px -apple-system, system-ui, sans-serif",
      });
      for (const [label, fn] of items) {
        const el = document.createElement("div");
        el.textContent = label;
        Object.assign(el.style, { padding: "8px 12px", cursor: "pointer", borderRadius: "6px" });
        el.onmouseenter = () => (el.style.background = "#3a3b3c");
        el.onmouseleave = () => (el.style.background = "");
        el.onclick = (ev) => { ev.stopPropagation(); closeMenu(); fn(); };
        ctxMenu.appendChild(el);
      }
      document.body.appendChild(ctxMenu);
      const r = ctxMenu.getBoundingClientRect();
      if (r.right > innerWidth) ctxMenu.style.left = innerWidth - r.width - 8 + "px";
      if (r.bottom > innerHeight) ctxMenu.style.top = innerHeight - r.height - 8 + "px";
      setTimeout(() => {
        document.addEventListener("click", closeMenu, true);
        document.addEventListener("scroll", closeMenu, true);
      }, 0);
    },
    true,
  );

  /* ----------------------------- Spell check ---------------------------- */
  const SPELL_SEL = '[contenteditable="true"], textarea, input[type="text"], input[type="search"]';
  function applySpellcheckNow() {
    const on = window.__CARRIER_SETTINGS__?.spellcheck !== false;
    document.querySelectorAll(SPELL_SEL).forEach((el) => el.setAttribute?.("spellcheck", on ? "true" : "false"));
  }
  function applySpellcheck() {
    applySpellcheckNow();
    new MutationObserver((muts) => {
      const on = window.__CARRIER_SETTINGS__?.spellcheck !== false;
      const set = (el) => el.setAttribute?.("spellcheck", on ? "true" : "false");
      for (const m of muts)
        for (const n of m.addedNodes)
          if (n.nodeType === 1) {
            if (n.matches?.(SPELL_SEL)) set(n);
            n.querySelectorAll?.(SPELL_SEL).forEach(set);
          }
    }).observe(document.documentElement, { childList: true, subtree: true });
  }
  // Re-apply when the Rust side pushes updated settings at runtime (no reload).
  window.addEventListener("carrier:settings", applySpellcheckNow);
  if (document.readyState === "loading")
    document.addEventListener("DOMContentLoaded", applySpellcheck);
  else applySpellcheck();

  /* --------------------- Native message notifications ------------------- */
  // Bridge the page's Web Notification API to native OS notifications so new
  // messages notify you even when Carrier is in the background.
  (function notificationBridge() {
    if (!window.__TAURI_INTERNALS__) return;
    // Keep the page convinced notifications are granted (below) so Facebook keeps
    // firing them; this also flips on the OS-level grant the native side needs.
    invoke("plugin:notification|is_permission_granted")
      ?.then?.((granted) => granted || invoke("plugin:notification|request_permission"))
      ?.catch?.(() => {});

    // Render the sender's avatar — Facebook puts its (remote fbcdn) URL on the
    // Notification's `icon` — to a small PNG data URL, so the native side can
    // attach it without re-fetching: the page already holds Facebook's session
    // and the cached image. Best-effort; resolves to "" if the image can't be
    // read (e.g. the canvas is tainted) and the notification then shows text only.
    const avatarToDataUrl = (url) =>
      new Promise((resolve) => {
        if (!url) return resolve("");
        const img = new Image();
        img.crossOrigin = "anonymous";
        let settled = false;
        const done = (v) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          resolve(v);
        };
        const timer = setTimeout(() => done(""), 2500);
        img.onload = () => {
          try {
            const size = 64;
            const c = document.createElement("canvas");
            c.width = size;
            c.height = size;
            c.getContext("2d").drawImage(img, 0, 0, size, size);
            done(c.toDataURL("image/png"));
          } catch (_) {
            done("");
          }
        };
        img.onerror = () => done("");
        img.src = url;
      });

    // Clicking a native notification routes back here by id: bring the
    // conversation up by invoking the original `onclick` Facebook assigned to its
    // Notification (that's what opens the right thread). A small bounded map keeps
    // those handlers alive between "notification shown" and "notification clicked".
    let notifySeq = 0;
    const notifyHandlers = new Map();
    window.__carrierNotifyClick = (id) => {
      const n = notifyHandlers.get(id);
      if (!n) return;
      notifyHandlers.delete(id);
      try {
        window.focus();
      } catch (_) {}
      try {
        // Facebook's onclick expects the click Event (it can read it / call
        // preventDefault); a native notification click carries no DOM event, so
        // hand it a synthetic one. Called as `n.onclick(...)` so `this` stays
        // bound to the Notification instance.
        n.onclick?.(new Event("click"));
      } catch (_) {}
    };

    function CarrierNotification(title, options = {}) {
      const opts = options || {};
      const s = window.__CARRIER_SETTINGS__ || {};
      // Only notify when Carrier is in the background — don't interrupt you with
      // a notification for a conversation you're actively reading — and never
      // while notifications are muted. (The auto-refresh nudge below still runs
      // when muted so a backgrounded window keeps catching up.)
      if (!s.mute_notifications && !document.hasFocus()) {
        const id = ++notifySeq;
        // Facebook assigns `this.onclick` right after construction; hold onto
        // this instance so the click route can call it. Cap the map so a long
        // session of unclicked notifications can't grow it without bound.
        notifyHandlers.set(id, this);
        if (notifyHandlers.size > 50)
          notifyHandlers.delete(notifyHandlers.keys().next().value);
        // Hide preview: replace the sender name and message text with a generic
        // notification, and skip the avatar so the sender's face never leaks.
        const hidePreview = s.hide_notification_preview;
        avatarToDataUrl(hidePreview ? "" : opts.icon).then((icon) => {
          // avatarToDataUrl is async (it decodes the image, up to ~2.5s), so
          // you may have returned to Carrier by the time it resolves — re-check
          // before emitting so we don't pop a notification for a conversation
          // you're now looking at. Drop the stored click handler too, since no
          // notification will reference it.
          if (document.hasFocus()) {
            notifyHandlers.delete(id);
            return;
          }
          invoke("plugin:event|emit", {
            event: "carrier:notify",
            payload: {
              id,
              title: hidePreview ? "Messenger" : String(title || "Messenger"),
              body: hidePreview ? "New message" : String(opts.body || ""),
              icon,
            },
          })?.catch?.(() => {});
        });
      }
      // Nudge the auto-refresh so the conversation view catches up even when
      // Facebook's in-WebView live sync stalls.
      try {
        window.__carrierOnNotification?.();
      } catch (_) {}
      this.title = title;
      this.onclick = null;
      this.close = () => {};
    }
    CarrierNotification.permission = "granted";
    CarrierNotification.requestPermission = (cb) => {
      if (cb) cb("granted");
      return Promise.resolve("granted");
    };
    try {
      Object.defineProperty(window, "Notification", { value: CarrierNotification, writable: true, configurable: true });
    } catch (_) {}
  })();

  /* --------------------------- Auto-refresh ----------------------------- */
  // Facebook's live message sync sometimes stalls inside a system WebView, so
  // the open conversation can lag behind. Reload to catch up: at least once per
  // new-message notification, plus a periodic refresh while in the background.
  // A reload is deferred while a message is half-typed so a draft is never lost.
  (function autoRefresh() {
    let pending = false;
    let timer = null;
    const composerHasText = () => {
      try {
        for (const el of document.querySelectorAll('[contenteditable="true"]')) {
          if ((el.textContent || "").trim().length > 0) return true;
        }
      } catch (_) {}
      return false;
    };
    const maybeReload = () => {
      if (!pending) return;
      // Never yank the page out from under a draft or an in-progress call.
      if (composerHasText() || window.__carrierInCall) {
        timer = setTimeout(maybeReload, 8000);
        return;
      }
      pending = false;
      location.reload();
    };
    const schedule = (delay) => {
      pending = true;
      clearTimeout(timer);
      timer = setTimeout(maybeReload, delay);
    };
    // Reload shortly after a new-message notification, but only while the window
    // is unfocused — that's when Facebook's live sync throttles and the view
    // goes stale. When you're actively reading, live sync works, so we leave the
    // page alone. (Debounced to batch a burst of notifications into one reload.)
    window.__carrierOnNotification = () => {
      if (!document.hasFocus()) schedule(4000);
    };
    // Regular refresh so an unfocused, stale window keeps catching up.
    setInterval(() => {
      if (!document.hasFocus()) schedule(2000);
    }, 4 * 60 * 1000);
  })();

  /* --------------------------- Force theme ------------------------------ */
  // Force the Messenger page theme to the user's choice (Settings → Theme). The
  // native window chrome is driven Rust-side from the same setting.
  (function forceTheme() {
    const html = document.documentElement;
    // Track the class we forced so switching back to "system" can undo it live
    // (settings re-apply on the same page without a reload — see carrier:settings).
    let forcedClass = null;
    const apply = () => {
      const forced = window.__CARRIER_SETTINGS__?.theme;
      if (forced !== "light" && forced !== "dark") {
        // "system": drop any class we previously forced, then leave FB alone.
        if (forcedClass) {
          html.classList.remove(forcedClass);
          forcedClass = null;
        }
        return;
      }
      const want = forced === "dark" ? "__fb-dark-mode" : "__fb-light-mode";
      const other = forced === "dark" ? "__fb-light-mode" : "__fb-dark-mode";
      if (!html.classList.contains(want) || html.classList.contains(other)) {
        html.classList.remove(other);
        html.classList.add(want);
      }
      forcedClass = want;
    };
    apply();
    window.addEventListener("carrier:settings", apply);
    // Re-assert if Facebook flips its own class back.
    new MutationObserver(apply).observe(html, { attributes: true, attributeFilter: ["class"] });
  })();

  /* --------------------------- Unread badge ----------------------------- */
  // Mirror the unread count onto the Dock / taskbar badge, and tell Rust so the
  // tray tooltip can show it too. The count is either unread *messages*
  // (Facebook's total, parsed from the "(N)" it puts in the page title) or
  // unread *conversations* (chats in the list rendered bold), per `badge_mode`.
  (function unreadBadge() {
    if (!window.__TAURI_INTERNALS__) return;

    // Unread messages: Facebook prefixes the page title with "(N)".
    const countUnreadMessages = () => {
      const m = (document.title || "").match(/\((\d+)\)/);
      return m ? parseInt(m[1], 10) : 0;
    };

    // Unread conversations: Facebook renders a chat's name/preview bold only
    // while it has unread messages. The class names are hashed and unstable, so
    // we key off the computed font-weight of each list row instead. Rows are the
    // links to a thread (`/t/<id>`); dedupe by thread id so a conversation that
    // also appears elsewhere (e.g. the open thread) isn't double-counted.
    const countUnreadConversations = () => {
      const seen = new Set();
      let n = 0;
      for (const a of document.querySelectorAll('a[href*="/t/"]')) {
        const m = (a.getAttribute("href") || "").match(/\/t\/(\d+)/);
        if (!m || seen.has(m[1])) continue;
        seen.add(m[1]);
        const row = a.closest('[role="row"]') || a;
        for (const span of row.querySelectorAll("span")) {
          const w = parseInt(getComputedStyle(span).fontWeight, 10) || 0;
          if (w >= 600 && (span.textContent || "").trim().length > 1) {
            n++;
            break;
          }
        }
      }
      return n;
    };

    let last = null;
    const setBadge = (n, force) => {
      if (n === last && !force) return;
      last = n;
      // NB: the command's argument is `value` (the Tauri `setter!` macro names
      // it that), not `count` — passing `count` silently clears the badge.
      invoke("plugin:window|set_badge_count", { value: n > 0 ? n : null })?.catch?.(() => {});
      invoke("plugin:event|emit", { event: "carrier:unread", payload: n })?.catch?.(() => {});
    };

    // `force` re-applies even when the count is unchanged — used for the initial
    // applications, which must survive the async macOS badge-authorization grant
    // (it lands shortly after launch) and the chat list's first render.
    const apply = (force) => {
      const s = window.__CARRIER_SETTINGS__ || {};
      if (s.unread_badge === false) {
        setBadge(0, force);
        return;
      }
      const conv = s.badge_mode === "conversations";
      const n = conv ? countUnreadConversations() : countUnreadMessages();
      // While Facebook is reloading the page, the title carries no "(N)" and the
      // chat list hasn't rendered yet, so both counts read 0. The OS keeps the
      // Dock badge across the reload on its own, so don't clear it during that
      // window — it would blink off and back. Only a "ready" page can be trusted
      // to mean 0 unread. (A non-zero count only happens once ready anyway.)
      const ready = conv
        ? document.querySelector('a[href*="/t/"]') !== null
        : /Messenger|Facebook/i.test(document.title || "");
      if (n === 0 && !ready) return;
      setBadge(n, force);
    };

    // Re-evaluate whenever the title changes — Facebook updates "(N)" the moment a
    // message arrives or is read, which is exactly when the unread count (and the
    // bolded conversations) change too, so this drives both modes promptly.
    // Observe <head> (not the <title> node directly) so it survives Facebook
    // replacing the element.
    let pending = false;
    const schedule = () => {
      if (pending) return;
      pending = true;
      setTimeout(() => {
        pending = false;
        apply(false); // snappy
        // Re-check shortly after: in conversation mode the (un)bolding of a row
        // can lag the title change by a frame or two.
        setTimeout(() => apply(false), 800);
      }, 120);
    };
    // This runs at document-start, where <head> may not exist yet; if so, wait
    // for it rather than permanently falling back to the interval.
    const headObserver = new MutationObserver(schedule);
    const observeHead = () => {
      if (!document.head) return false;
      headObserver.observe(document.head, { childList: true, subtree: true, characterData: true });
      return true;
    };
    if (!observeHead()) {
      const waitForHead = new MutationObserver(() => {
        if (observeHead()) waitForHead.disconnect();
      });
      waitForHead.observe(document.documentElement, { childList: true, subtree: true });
    }
    window.addEventListener("carrier:settings", () => apply(true));
    setInterval(() => apply(false), 5000);
    apply(true);
    setTimeout(() => apply(true), 1500);
    setTimeout(() => apply(true), 4000);
  })();

  /* --------------------------- Compact mode ----------------------------- */
  // Toggle a marker class the injected CSS keys off of to collapse side panels.
  (function compact() {
    const apply = () => {
      document.documentElement.toggleAttribute(
        "data-carrier-compact",
        window.__CARRIER_SETTINGS__?.compact === true,
      );
    };
    apply();
    window.addEventListener("carrier:settings", apply);
  })();

  /* ----------------------- Hide names & avatars ------------------------- */
  // Toggle a marker attribute the injected CSS keys off of to blur contact
  // names and avatars (Settings / View ▸ Hide Names & Avatars / Cmd+Shift+N).
  (function hideNames() {
    const apply = () => {
      document.documentElement.toggleAttribute(
        "data-carrier-hide-names",
        window.__CARRIER_SETTINGS__?.hide_names_avatars === true,
      );
    };
    apply();
    window.addEventListener("carrier:settings", apply);
  })();

  /* ------------------ Camera/mic permission warning --------------------- */
  // If a call can't get the camera or mic because the OS blocked it, tell the
  // user and offer to open the OS privacy settings.
  (function permissionWarning() {
    const md = navigator.mediaDevices;
    if (!md || !md.getUserMedia) return;
    const original = md.getUserMedia.bind(md);
    md.getUserMedia = async function (constraints) {
      try {
        const stream = await original(constraints);
        // Track the call so the auto-refresh doesn't reload mid-call.
        window.__carrierInCall = true;
        const tracks = stream.getTracks();
        let live = tracks.length;
        tracks.forEach((t) =>
          t.addEventListener("ended", () => {
            if (--live <= 0) window.__carrierInCall = false;
          }),
        );
        return stream;
      } catch (err) {
        if (err && (err.name === "NotAllowedError" || err.name === "NotFoundError")) {
          const kind = constraints && constraints.video ? "camera" : "microphone";
          toast(`Carrier needs ${kind} access — check System Settings → Privacy & Security`);
          // macOS deep link to the relevant privacy pane (no-op elsewhere).
          const pane = kind === "camera" ? "Privacy_Camera" : "Privacy_Microphone";
          openUrl(`x-apple.systempreferences:com.apple.preference.security?${pane}`);
        }
        throw err;
      }
    };
  })();

  /* ----------------------- Login page tidy-up --------------------------- */
  // On the logged-out page, hide Facebook's marketing collage and centre the
  // login box, so the window shows just the login form.
  (function loginTidy() {
    const HIDE = "data-carrier-hide";
    const COL = "data-carrier-login-col";
    const ANC = "data-carrier-login-anc";
    let scheduled = false;

    const prefersDark = () => window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    // Follow the forced theme (Settings → Theme) when set, else the system. FB's
    // login page ships only a light theme, so this drives our dark swap.
    const wantDark = () => {
      const t = window.__CARRIER_SETTINGS__?.theme;
      if (t === "dark") return true;
      if (t === "light") return false;
      return prefersDark();
    };
    // A near-opaque light fill (Facebook's login wrappers) we want to clear so
    // the dark backdrop shows through.
    const isLightFill = (bg) => {
      const m = bg && bg.match(/rgba?\(([^)]+)\)/);
      if (!m) return false;
      const [r, g, b, a = 1] = m[1].split(",").map((s) => parseFloat(s));
      return a > 0.9 && (r + g + b) / 3 > 200;
    };

    // Only Facebook's own login page — not the in-app OAuth provider pages
    // (Google/Apple/Microsoft), which also have password fields.
    const onFacebook = () => /(^|\.)facebook\.com$/i.test(location.hostname);

    function tidy() {
      const html = document.documentElement;
      // The login page has both an identifier and a password field. Checkpoint /
      // re-auth / recovery forms have only a password field, so require both to
      // avoid hiding their required UI.
      const pass = document.querySelector('input[name="pass"]');
      const isLogin = onFacebook() && !!pass && !!document.querySelector('input[name="email"]');
      if (!isLogin) {
        if (html.hasAttribute("data-carrier-login")) {
          html.removeAttribute("data-carrier-login");
          document.querySelectorAll("[" + HIDE + "]").forEach((el) => el.removeAttribute(HIDE));
          document.querySelectorAll("[" + COL + "]").forEach((el) => el.removeAttribute(COL));
          document.querySelectorAll("[" + ANC + "]").forEach((el) => el.removeAttribute(ANC));
          // Undo our login dark swap so the logged-in app keeps FB's own theme.
          if (html.hasAttribute("data-carrier-darkswap")) {
            html.classList.replace("__fb-dark-mode", "__fb-light-mode");
            html.removeAttribute("data-carrier-darkswap");
          }
        }
        return;
      }
      html.setAttribute("data-carrier-login", "");
      // Use Facebook's native dark palette on the login page when the system is
      // dark (the login page itself ships only a light theme). Reacts to the
      // system theme changing while the login screen is open.
      const dark = wantDark();
      if (dark && html.classList.contains("__fb-light-mode")) {
        html.classList.replace("__fb-light-mode", "__fb-dark-mode");
        html.setAttribute("data-carrier-darkswap", "");
      } else if (!dark && html.hasAttribute("data-carrier-darkswap")) {
        html.classList.replace("__fb-dark-mode", "__fb-light-mode");
        html.removeAttribute("data-carrier-darkswap");
      }
      const form = pass.closest("form");
      if (!form) return;
      // Climb to the column that holds the login card (the widest box that
      // still isn't basically the full viewport width).
      let col = form;
      while (
        col.parentElement &&
        col.parentElement !== document.body &&
        col.parentElement.getBoundingClientRect().width < window.innerWidth * 0.92
      ) {
        col = col.parentElement;
      }
      if (!col.hasAttribute(COL)) col.setAttribute(COL, "");
      // Hide every sibling of the login column, up the ancestor chain, and mark
      // the ancestor wrappers so their (often white) backgrounds can be cleared.
      let node = col;
      while (node && node.parentElement && node !== document.body) {
        for (const sib of node.parentElement.children) {
          if (sib !== node && !sib.hasAttribute(HIDE) && !sib.hasAttribute(COL)) {
            sib.setAttribute(HIDE, "");
          }
        }
        if (node !== col && !node.hasAttribute(ANC)) node.setAttribute(ANC, "");
        node = node.parentElement;
      }
      // Belt-and-braces: clear any large opaque-light backdrop the ancestor walk
      // didn't catch, so nothing white surrounds the (dark) login card. Undo it
      // first so switching back to light/system restores the white backgrounds.
      for (const el of document.querySelectorAll("[data-carrier-cleared-bg]")) {
        el.style.removeProperty("background-color");
        el.removeAttribute("data-carrier-cleared-bg");
      }
      if (dark) {
        for (const el of document.body.querySelectorAll("*")) {
          const r = el.getBoundingClientRect();
          if (
            r.width >= window.innerWidth * 0.6 &&
            r.height >= window.innerHeight * 0.5 &&
            isLightFill(getComputedStyle(el).backgroundColor)
          ) {
            el.setAttribute("data-carrier-cleared-bg", "");
            el.style.setProperty("background-color", "transparent", "important");
          }
        }
      }
    }

    const schedule = () => {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(() => {
        scheduled = false;
        try {
          tidy();
        } catch (_) {}
      });
    };
    schedule();
    new MutationObserver(schedule).observe(document.documentElement, { childList: true, subtree: true });
    window.addEventListener("carrier:settings", schedule);
    if (window.matchMedia) {
      window.matchMedia("(prefers-color-scheme: dark)").addEventListener?.("change", schedule);
    }
  })();

  /* -------------------- Image / video zoom + pan viewer ----------------- */
  // Double-click a message image or video to enter a zoom/pan overlay:
  //   wheel = zoom, drag or arrow keys = pan, Esc / click-away = exit.
  (function mediaViewer() {
    const MIN = 1, MAX = 8, STEP = 1.15, PAN = 40;
    let target = null, scale = 1, tx = 0, ty = 0;
    let active = false, dragging = false;
    let sx = 0, sy = 0, stx = 0, sty = 0;

    function pickTarget(e) {
      const video = e.target.closest("video") || e.target.closest("div")?.querySelector("video");
      if (video) return video;
      const img = e.target.closest("img[alt]");
      if (!img) return null;
      const src = img.currentSrc || img.src || "";
      // Skip emoji / sticker sprites and data URIs.
      if (src.startsWith("data:") || src.includes("stp=dst-png_s")) return null;
      return img;
    }

    function render(animated = true) {
      if (!target) return;
      const reset = scale === 1 && tx === 0 && ty === 0;
      target.style.transition = !animated || dragging ? "none" : "transform .15s cubic-bezier(0,0,.2,1)";
      target.style.transformOrigin = "center center";
      target.style.zIndex = reset ? "" : "1000";
      target.style.maxWidth = reset ? "" : "none";
      target.style.maxHeight = reset ? "" : "none";
      target.style.transform = reset ? "" : `translate(${tx}px,${ty}px) scale(${scale})`;
      target.style.cursor = reset ? "zoom-in" : dragging ? "grabbing" : "grab";
    }

    function exit() {
      if (!active) return;
      active = false;
      handlers.forEach(([t, f, o]) => document.removeEventListener(t, f, o));
      if (target) {
        target.style.cssText = target.style.cssText
          .replace(/transform[^;]*;?/g, "")
          .replace(/transition[^;]*;?/g, "")
          .replace(/max-(width|height)[^;]*;?/g, "")
          .replace(/z-index[^;]*;?/g, "")
          .replace(/cursor[^;]*;?/g, "");
      }
      target = null; scale = 1; tx = 0; ty = 0; dragging = false;
    }

    const onWheel = (e) => {
      if (!target) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const r = target.getBoundingClientRect();
      const prev = scale;
      scale = e.deltaY < 0 ? Math.min(MAX, scale * STEP) : Math.max(MIN, scale / STEP);
      if (scale <= 1) { tx = 0; ty = 0; }
      else {
        const k = scale / prev;
        tx += (e.clientX - (r.left + r.width / 2)) * (1 - k);
        ty += (e.clientY - (r.top + r.height / 2)) * (1 - k);
      }
      render();
    };
    const onDown = (e) => {
      if (e.button !== 0 || scale <= 1 || !target?.contains(e.target)) return;
      dragging = true; sx = e.clientX; sy = e.clientY; stx = tx; sty = ty;
      e.preventDefault(); e.stopImmediatePropagation();
    };
    const onMove = (e) => {
      if (!dragging) return;
      tx = stx + (e.clientX - sx); ty = sty + (e.clientY - sy); render();
    };
    const onUp = () => { dragging = false; render(); };
    const onKey = (e) => {
      if (e.key === "Escape") return exit();
      const d = { ArrowLeft: [PAN, 0], ArrowRight: [-PAN, 0], ArrowUp: [0, PAN], ArrowDown: [0, -PAN] }[e.key];
      if (d && scale > 1) { e.preventDefault(); e.stopImmediatePropagation(); tx += d[0]; ty += d[1]; render(); }
    };
    const onClick = (e) => { if (active && target && !target.contains(e.target)) exit(); };

    const handlers = [
      ["wheel", onWheel, { passive: false, capture: true }],
      ["mousedown", onDown, { capture: true }],
      ["mousemove", onMove, { capture: true }],
      ["mouseup", onUp, { capture: true }],
      ["keydown", onKey, { capture: true }],
      ["click", onClick, { capture: true }],
    ];

    document.addEventListener(
      "dblclick",
      (e) => {
        const t = pickTarget(e);
        if (!t) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        if (active) return exit();
        active = true; target = t;
        const r = t.getBoundingClientRect();
        scale = 2;
        tx = (e.clientX - (r.left + r.width / 2)) * (1 - scale);
        ty = (e.clientY - (r.top + r.height / 2)) * (1 - scale);
        render(false);
        handlers.forEach(([type, f, o]) => document.addEventListener(type, f, o));
      },
      { capture: true },
    );
  })();

  /* --------------------------- Fullscreen polyfill ---------------------- */
  // Some WebViews don't implement the Fullscreen API the way FB's video player
  // expects. Emulate it by promoting the element to a fixed, full-window layer.
  (function fullscreenPolyfill() {
    if (document.fullscreenEnabled && Element.prototype.requestFullscreen) return;
    let current = null;
    const enter = (el) => {
      current = el;
      el.dataset.carrierPrevStyle = el.getAttribute("style") || "";
      Object.assign(el.style, {
        position: "fixed", inset: "0", width: "100vw", height: "100vh",
        zIndex: "2147483647", background: "#000",
      });
      document.dispatchEvent(new Event("fullscreenchange"));
      return Promise.resolve();
    };
    const leave = () => {
      if (current) {
        current.setAttribute("style", current.dataset.carrierPrevStyle || "");
        delete current.dataset.carrierPrevStyle;
        current = null;
        document.dispatchEvent(new Event("fullscreenchange"));
      }
      return Promise.resolve();
    };
    Object.defineProperty(document, "fullscreenElement", { get: () => current, configurable: true });
    Element.prototype.requestFullscreen = function () { return enter(this); };
    Element.prototype.webkitRequestFullscreen = Element.prototype.requestFullscreen;
    document.exitFullscreen = leave;
    document.webkitExitFullscreen = leave;
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && current) leave(); }, true);
  })();
})();
