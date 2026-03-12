// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Kyomi Analytics — Lightweight event collection snippet
 *
 * Usage (signed key — recommended for multi-tenant):
 *   <script defer src="https://analytics.kyomi.ai/k.js" data-key="SIGNED_KEY"></script>
 *
 * Usage (legacy — kyomi.ai dogfooding only):
 *   <script defer src="https://analytics.kyomi.ai/k.js" data-site="SITE_ID"></script>
 *
 * Automatically tracks pageviews including SPA navigation.
 * ~1KB gzipped.
 */
(function () {
  "use strict";

  var script = document.currentScript;
  var key = script && script.getAttribute("data-key");
  var siteId = script && script.getAttribute("data-site");
  if (!key && !siteId) return;

  // Resolve the collect endpoint relative to where the script is hosted
  var endpoint = new URL(script.src).origin + "/api/collect";

  var blocked = false;
  try { blocked = !!sessionStorage.getItem("_kb"); } catch (e) {}

  var lastPathname = "";
  var userId = "";
  var identified = false;

  function send(eventName, props) {
    // Don't send if rate-limited (429 received earlier in this session)
    if (blocked) return;
    // Don't track if the page is prerendering
    if (document.visibilityState === "prerender") return;

    var payload = {
      n: eventName || "pageview",
      u: location.href,
      r: document.referrer,
      w: window.innerWidth,
      h: window.innerHeight,
    };

    if (key) {
      payload.key = key;
    } else {
      payload.s = siteId;
    }
    if (userId) payload.uid = userId;
    if (props) payload.p = props;
    if (identified) payload.i = 1;

    // Use text/plain content type to avoid CORS preflight (same approach as Plausible).
    // The server parses JSON from the body regardless of content type.
    // We use fetch() instead of sendBeacon() so we can read the response status
    // and stop sending on 429 (rate limit / quota exceeded).
    fetch(endpoint, {
      method: "POST",
      body: JSON.stringify(payload),
      headers: { "Content-Type": "text/plain" },
      keepalive: true,
    }).then(function (res) {
      if (res.status === 429) {
        blocked = true;
        try { sessionStorage.setItem("_kb", "1"); } catch (e) {}
      }
    }).catch(function () {});
  }

  function trackPageview() {
    // Deduplicate — don't re-track the same pathname
    if (location.pathname === lastPathname) return;
    lastPathname = location.pathname;
    send("pageview");
  }

  // Track initial pageview
  trackPageview();

  // Track SPA navigation — intercept pushState/replaceState
  var originalPushState = history.pushState;
  var originalReplaceState = history.replaceState;

  history.pushState = function () {
    originalPushState.apply(this, arguments);
    trackPageview();
  };

  history.replaceState = function () {
    originalReplaceState.apply(this, arguments);
    trackPageview();
  };

  // Track browser back/forward navigation
  window.addEventListener("popstate", trackPageview);

  // Expose global API
  window.kyomi = {
    track: function (event, props) {
      send(event, props);
    },
    identify: function (uid) {
      userId = uid || "";
      identified = true;
    },
  };
})();
