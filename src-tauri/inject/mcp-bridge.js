/* ===================================================================== *
 *  tauri-mcp guest bridge  —  DEV ONLY (compiled only with `--features mcp`)
 * ===================================================================== *
 *
 * tauri-plugin-mcp drives the webview by *emitting* Tauri events to it
 * (`execute-js`, `got-dom-content`, …) and waiting for a correlated
 * `*-response-<uuid>` event back. The responder that listens for those events
 * ships in the plugin's `guest-js`, which a normal Tauri app imports into its
 * frontend bundle. Carrier's main window is a REMOTE origin (facebook.com), so
 * it never loads that guest-js — which is why every round-trip MCP command
 * (execute_js, get_dom) times out with "Timeout waiting for … response".
 *
 * This file is the missing responder, hand-rolled against the low-level
 * `__TAURI_INTERNALS__` API so it works without `withGlobalTauri` (Carrier keeps
 * that off so Facebook can't see `window.__TAURI__`). It is injected only in
 * `mcp`/debug builds, so release builds never expose a JS-eval responder.
 *
 * Wire protocol (verified against tauri 2.11.3 + tauri-plugin-mcp d5e0b80):
 *   • Rust emits to the webview via `app.emit_to("<label>", "<event>", code)`,
 *     which is `EventTarget::AnyLabel{label}` — matches a listener registered
 *     with `{kind:"WebviewWindow", label}` (NOT `{kind:"Any"}`).
 *   • The plugin wraps a non-object payload as `{_payload, _correlationId}`.
 *   • The reply is a plain global `emit("<event>-response-<id>", data)`, which
 *     reaches the Rust `app.once(...)` listener (target Any).
 */
(function () {
  if (window.__CARRIER_MCP_BRIDGE__) return;

  function safeStringify(v) {
    try {
      return JSON.stringify(v);
    } catch (_) {
      try {
        return String(v);
      } catch (__) {
        return "[unserializable]";
      }
    }
  }

  function setup() {
    var II = window.__TAURI_INTERNALS__;
    if (!II || typeof II.invoke !== "function" || typeof II.transformCallback !== "function") {
      return false;
    }

    // The window this script runs in; Rust emits round-trip events to its label.
    var meta = II.metadata || {};
    var label =
      (meta.currentWebview && meta.currentWebview.label) ||
      (meta.currentWindow && meta.currentWindow.label) ||
      "main";
    var target = { kind: "WebviewWindow", label: label };

    function listen(event, handler) {
      // Rust's plugin:event|listen populates its own JS listener registry, so we
      // only have to hand it a transformCallback id. The handler is invoked with
      // `{event, id, payload}`.
      II.invoke("plugin:event|listen", {
        event: event,
        target: target,
        handler: II.transformCallback(handler),
      });
    }

    function emit(event, payload) {
      return II.invoke("plugin:event|emit", { event: event, payload: payload });
    }

    function correlationId(p) {
      return p && typeof p === "object" && typeof p._correlationId === "string"
        ? p._correlationId
        : null;
    }

    function respond(baseEvent, cid, data) {
      emit(cid ? baseEvent + "-" + cid : baseEvent, data);
    }

    // --- execute-js : the universal escape hatch -----------------------------
    listen("execute-js", function (ev) {
      var p = ev && ev.payload;
      var cid = correlationId(p);
      var reply = function (result) {
        respond("execute-js-response", cid, {
          result: typeof result === "object" ? safeStringify(result) : String(result),
          type: typeof result,
        });
      };
      try {
        // emit_and_wait wraps a non-object payload (the code string) as _payload.
        var code = p && p._payload !== undefined ? p._payload : p;
        var result;
        try {
          // Expression form first (so the last value is returned)…
          result = new Function("return (" + code + ")")();
        } catch (_) {
          // …falling back to statement form.
          result = new Function(code)();
        }
        // Resolve thenables so `await fetch(...)`-style snippets return real data.
        if (result && typeof result.then === "function") {
          result.then(reply, function (e) {
            respond("execute-js-response", cid, {
              result: null,
              type: "error",
              error: String((e && e.stack) || e),
            });
          });
        } else {
          reply(result);
        }
      } catch (e) {
        respond("execute-js-response", cid, {
          result: null,
          type: "error",
          error: String((e && e.stack) || e),
        });
      }
    });

    // --- got-dom-content : full serialized DOM (no eval; CSP-safe) ------------
    listen("got-dom-content", function (ev) {
      var cid = correlationId(ev && ev.payload);
      var dom = "";
      try {
        if (document.readyState === "complete" || document.readyState === "interactive") {
          dom = document.documentElement.outerHTML;
        }
      } catch (_) {}
      respond("got-dom-content-response", cid, dom);
    });

    window.__CARRIER_MCP_BRIDGE__ = true;
    try {
      console.log("[carrier] tauri-mcp guest bridge ready on window '" + label + "'");
    } catch (_) {}
    return true;
  }

  // __TAURI_INTERNALS__ is normally present at document-start, but retry briefly
  // in case this init script runs a touch early.
  if (!setup()) {
    var tries = 0;
    var timer = setInterval(function () {
      if (setup() || ++tries > 100) clearInterval(timer);
    }, 50);
  }
})();
