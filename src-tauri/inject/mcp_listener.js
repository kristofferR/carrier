/*
 * Test-only: a self-contained listener that lets tauri-plugin-mcp's `execute_js`
 * run code in this (remote) page. Mirrors the plugin's frontend bridge but uses
 * window.__TAURI__ so it needs no ESM imports. Compiled in ONLY with the `mcp`
 * Cargo feature (autonomous testing); never present in release builds.
 */
(function () {
  "use strict";
  var T = window.__TAURI__;
  if (!T || !T.event) return;

  function correlationId(p) {
    return p && typeof p === "object" && typeof p._correlationId === "string" ? p._correlationId : null;
  }
  function emitResponse(base, cid, data) {
    return T.event.emit(cid ? base + "-" + cid : base, data);
  }
  function exec(code) {
    try {
      return new Function("return (" + code + ")")();
    } catch (_) {
      return new Function(code)();
    }
  }
  function onExecute(event) {
    var p = event.payload;
    var cid = correlationId(p);
    try {
      var code = typeof p === "object" && p._payload !== undefined ? p._payload : p;
      // Diagnostic sentinels that read the DOM directly (no eval -> not blocked
      // by the remote page's CSP), so we can inspect what the page actually is.
      var result;
      if (code === "__CARRIER_DOM__") {
        result = document.documentElement.outerHTML.length + "|" + document.documentElement.outerHTML.slice(0, 4000);
      } else if (code === "__CARRIER_INFO__") {
        result =
          "title=" + document.title +
          " | bodyLen=" + (document.body ? document.body.innerHTML.length : -1) +
          " | text=" + (document.body ? document.body.innerText.slice(0, 400) : "") +
          " | inputs=" + document.querySelectorAll("input").length +
          " | hasPass=" + !!document.querySelector('input[type=password],input[name=pass]') +
          " | loginAttr=" + document.documentElement.hasAttribute("data-carrier-login") +
          " | styleCarrier=" + !!document.querySelector("style[data-carrier]") +
          " | readyState=" + document.readyState;
      } else if (code === "__CARRIER_BG__") {
        var gc = function (el) {
          return el ? getComputedStyle(el).backgroundColor : "none";
        };
        var b = document.body;
        var kids = b ? Array.prototype.slice.call(b.children, 0, 6) : [];
        result =
          "html=" + gc(document.documentElement) +
          " body=" + gc(b) +
          " kids=" + kids.map(function (c) { return c.tagName + "." + (c.className || "").toString().slice(0, 20) + "=" + gc(c); }).join(" | ");
      } else {
        result = exec(code);
      }
      emitResponse("execute-js-response", cid, {
        result: typeof result === "object" ? JSON.stringify(result) : String(result),
        type: typeof result,
      });
    } catch (err) {
      emitResponse("execute-js-response", cid, { result: null, type: "error", error: String(err) });
    }
  }

  // Prefer the webview-window listener (window-targeted), fall back to global.
  var w = T.webviewWindow && T.webviewWindow.getCurrentWebviewWindow && T.webviewWindow.getCurrentWebviewWindow();
  if (w && w.listen) w.listen("execute-js", onExecute);
  else T.event.listen("execute-js", onExecute);
})();
