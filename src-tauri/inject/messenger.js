/*
 * Carrier — in-page enhancements for the Messenger web app.
 * Clean-room implementation (keyboard shortcuts, page zoom, an image/video
 * zoom + pan viewer, and a fullscreen polyfill for the WebView).
 *
 * Runs as a WebView initialization script at document start.
 */
(function () {
  "use strict";

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
