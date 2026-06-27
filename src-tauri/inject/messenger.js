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
  const notify = (options) =>
    invoke("plugin:notification|notify", { options })?.catch?.(() => {});

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
      }
    },
    true,
  );

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
    let href = src;
    // Re-wrap remote media as a same-origin blob so the `download` attribute is
    // honoured (it's ignored for cross-origin URLs) and the save keeps its name.
    if (!src.startsWith("blob:") && !src.startsWith("data:")) {
      const blob = await (await fetch(src)).blob();
      if (blob.size > MAX_BLOB) throw new Error("file too large");
      href = URL.createObjectURL(blob);
    }
    const a = document.createElement("a");
    a.href = href;
    a.download = filenameFromUrl(src) || fallbackName;
    a.style.display = "none";
    document.body.appendChild(a);
    a.click();
    a.remove();
    if (href !== src) setTimeout(() => URL.revokeObjectURL(href), 10000);
  }

  async function copyImageSrc(src) {
    const blob = await (await fetch(src)).blob();
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
    // Make sure the OS has actually granted Carrier notification permission,
    // otherwise plugin:notification|notify silently no-ops.
    invoke("plugin:notification|is_permission_granted")
      ?.then?.((granted) => granted || invoke("plugin:notification|request_permission"))
      ?.catch?.(() => {});
    function CarrierNotification(title, options = {}) {
      try {
        notify({ title: String(title || "Messenger"), body: String(options.body || "") });
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

  /* ------------------------- Theme sync (native) ------------------------ */
  // Keep the native window chrome in step with the page's light/dark theme.
  (function themeSync() {
    if (!window.__TAURI_INTERNALS__ || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const push = () => {
      // Core window command (works from the remote origin); applies to the
      // calling webview's own window.
      invoke("plugin:window|set_theme", { value: mq.matches ? "dark" : "light" })?.catch?.(() => {});
    };
    push();
    mq.addEventListener?.("change", push);
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
        return await original(constraints);
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
      const dark = prefersDark();
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
