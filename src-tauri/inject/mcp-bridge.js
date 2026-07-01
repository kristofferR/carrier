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
        // CSP-safe, sanitized selector probe for Hide Names & Avatars work.
        if (code === "__carrier_mcp_privacy_probe__") {
          reply(privacyProbe());
          return;
        }
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

    // Keep privacy diagnostics sanitized: no message text, raw thread/profile
    // IDs, image URLs, or alt/aria-label contents.
    function privacyProbe() {
      var THREAD_ROW_SEL = '[role="grid"] a[href*="/t/"], [role="navigation"] a[href*="/t/"]';
      var TEXT_SURFACE_SEL = "span, div, h1, h2, h3, h4";
      var VISUAL_SEL = 'img, svg, image, [style*="background-image"]';
      var IDENTITY_SEL = "[data-carrier-private-identity]";
      var WRAPPER_SEL = "[data-carrier-private-wrapper]";
      var PREVIEW_NAME_RE = /^([^:]{1,40}):(?=\s|$)/;
      var PREVIEW_EVENT_RE =
        /^(.{1,40}?)(?=\s+(?:sent|replied|reacted|liked|laughed|loved|mentioned|shared|left|joined|added|removed|changed|created|named|started)\b)/i;

      function rect(el) {
        var r = el.getBoundingClientRect();
        return {
          x: Math.round(r.x),
          y: Math.round(r.y),
          w: Math.round(r.width),
          h: Math.round(r.height),
        };
      }

      function visible(el) {
        var r = el.getBoundingClientRect();
        var cs = getComputedStyle(el);
        return r.width > 0 && r.height > 0 && cs.display !== "none" && cs.visibility !== "hidden";
      }

      function maskedHref(el) {
        var href = el && el.getAttribute && el.getAttribute("href");
        if (!href) return "";
        return href.replace(/\d{3,}/g, "{id}");
      }

      function textLength(el) {
        return (el.textContent || "").replace(/\s+/g, " ").trim().length;
      }

      function attrs(el) {
        var out = {};
        Array.prototype.forEach.call(el.attributes || [], function (attr) {
          if (attr.name === "src" || attr.name === "alt" || attr.name === "aria-label") {
            out[attr.name] = attr.value ? "[present]" : "";
          } else if (attr.name === "href") {
            out.href = maskedHref(el);
          } else if (attr.value.length < 80) {
            out[attr.name] = attr.value;
          } else {
            out[attr.name] = "[long]";
          }
        });
        return out;
      }

      function ancestors(el) {
        var out = [];
        for (var n = el.parentElement; n && out.length < 5; n = n.parentElement) {
          out.push({
            tag: n.tagName.toLowerCase(),
            role: n.getAttribute("role") || "",
            href: maskedHref(n),
            aria: n.getAttribute("aria-label") ? "[present]" : "",
            style: n.getAttribute("style") ? "[present]" : "",
            className: n.getAttribute("class") ? "[present]" : "",
            carrierIdentity: n.hasAttribute("data-carrier-private-identity"),
            carrierWrapper: n.hasAttribute("data-carrier-private-wrapper"),
            rect: rect(n),
          });
        }
        return out;
      }

      function item(el) {
        var cs = getComputedStyle(el);
        var closestHref = el.closest("a[href]");
        var text = (el.textContent || "").replace(/\s+/g, " ").trim();
        return {
          tag: el.tagName.toLowerCase(),
          role: el.getAttribute("role") || "",
          href: maskedHref(el),
          aria: el.getAttribute("aria-label") ? "[present]" : "",
          textLength: text.length,
          rect: rect(el),
          filter: cs.filter || "",
          backgroundImage: cs.backgroundImage && cs.backgroundImage !== "none" ? "[present]" : "",
          closestHref: maskedHref(closestHref),
          attrs: attrs(el),
          ancestors: ancestors(el),
          flags: {
            inArticle: !!el.closest('[role="article"]'),
            inHeading: !!el.closest("h1,h2,h3,h4"),
            hasReplyPhrase: /\breplied to\b/i.test(text),
            previewNamePattern: PREVIEW_NAME_RE.test(text),
            previewEventPattern: PREVIEW_EVENT_RE.test(text),
            carrierIdentity: el.hasAttribute("data-carrier-private-identity"),
            carrierIdentityAncestor: !!el.closest(IDENTITY_SEL),
            carrierWrapper: el.hasAttribute("data-carrier-private-wrapper"),
            carrierWrapperAncestor: !!el.closest(WRAPPER_SEL),
          },
        };
      }

      function list(selector, limit, root) {
        var out = [];
        (root || document).querySelectorAll(selector).forEach(function (el) {
          if (out.length >= limit || !visible(el)) return;
          out.push(item(el));
        });
        return out;
      }

      function textLeaves(root, limit) {
        var out = [];
        root.querySelectorAll(TEXT_SURFACE_SEL).forEach(function (el) {
          if (out.length >= limit || !visible(el)) return;
          if (!textLength(el)) return;
          var hasTextChild = false;
          Array.prototype.forEach.call(el.children || [], function (child) {
            if (textLength(child)) hasTextChild = true;
          });
          if (hasTextChild) return;
          out.push(item(el));
        });
        return out.sort(function (a, b) {
          return a.rect.y - b.rect.y || a.rect.x - b.rect.x;
        });
      }

      function visuals(root, limit) {
        return list(VISUAL_SEL, limit, root).sort(function (a, b) {
          return a.rect.y - b.rect.y || a.rect.x - b.rect.x;
        });
      }

      function rows(limit) {
        var seen = {};
        var out = [];
        document.querySelectorAll(THREAD_ROW_SEL).forEach(function (row) {
          var href = row.getAttribute("href") || "";
          if (out.length >= limit || seen[href] || !visible(row)) return;
          seen[href] = true;
          out.push({
            row: item(row),
            textLeaves: textLeaves(row, 6),
            visuals: visuals(row, 4),
            identityMarkers: list(IDENTITY_SEL, 10, row),
            wrapperMarkers: list(WRAPPER_SEL, 8, row),
          });
        });
        return out;
      }

      var mainRoot = document.querySelector('[role="main"]') || document.querySelector("main");

      return {
        url: location.href.replace(/\d{3,}/g, "{id}"),
        hasHideAttr: document.documentElement.hasAttribute("data-carrier-hide-names"),
        selectors: {
          conversationRows: rows(5),
          identityMarkers: list(IDENTITY_SEL, 40),
          wrapperMarkers: list(WRAPPER_SEL, 30),
          mainProfileLinks: list(':is(main, [role="main"]) a[href^="/"][href$="/"]:not([href*="/messages/"])', 12),
          circularAvatars: list(':is(main, [role="main"]) img[referrerpolicy="origin-when-cross-origin"][style*="border-radius: 50%"]', 20),
          readReceipts: list(':is(main, [role="main"]) [role="article"] img[height="14"][width="14"][tabindex="-1"]', 20),
          mainImages: list(':is(main, [role="main"]) img', 30),
          senderHeadings: list(':is(main, [role="main"]) [role="article"] h3, :is(main, [role="main"]) [role="article"] h3 *', 40),
          replyAttribution: (mainRoot ? textLeaves(mainRoot, 200) : [])
            .filter(function (el) {
              return el.flags.hasReplyPhrase;
            })
            .slice(0, 20),
        },
      };
    }

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
