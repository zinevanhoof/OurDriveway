import { isTauri } from "@tauri-apps/api/core";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";

// Native (Tauri) talks to the backend through plugin-http's Rust reqwest client;
// web uses the browser's fetch. The split exists because reqwest ignores CORS and
// SameSite, so the refresh cookie rides along cross-origin — that's the whole point
// of keeping the frontend bundled locally (offline shell + atomic app updates).
// Safe in a web-only build: isTauri() is a plain boolean check that returns false.
//
// **Never hardcode this.** It is a capability check, not a design preference, and
// `installNativeFetch` below replaces `window.fetch` on the strength of it — forcing it
// true in a browser would route every request through a plugin that isn't there and break
// the whole app. If you want the phone *layout* everywhere, that is `MOBILE_SHELL` in
// App.vue, which is a separate question deliberately kept separate.
//
// Its other callers are capability decisions too: the platform geolocation prompt
// (lib/geo.ts), opening a maps app (BookedSpotRow), and choosing an `ourdriveway://`
// return URL over an http one for a redirect payment.
export const native = isTauri();

// reqwest can't resolve a relative URL and rejects the `tauri://` scheme a bundled
// build loads from, so native needs an absolute base. Both native and web route
// through Caddy, so the base is Caddy's origin.
// ponytail: hardcoded to match the rest of the app. If the host ever needs to vary,
// this one const is the single place to add a VITE_API_URL fallback.
const apiBase = "http://192.168.50.29";

// Replace window.fetch once, at startup, so every caller (king.ts REST + urql) stays
// on plain relative-URL fetch and is routed correctly without knowing about Tauri.
// Web is left untouched: relative URLs resolve same-origin against Caddy.
export function installNativeFetch() {
  if (!native) return;

  // The real webview fetch — Tauri's own IPC rides window.fetch and must keep using it.
  const browserFetch = window.fetch.bind(window);

  window.fetch = async (input, init) => {
    // Resolve the relative path against Caddy's origin; an already-absolute URL
    // (new URL ignores the base then) passes through unchanged.
    const raw =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    const parsed = new URL(raw, apiBase);

    // Tauri's IPC (Channel callbacks / plugin invoke on Android) posts to
    // http://ipc.localhost — an internal protocol, NOT a real host. Routing it
    // through plugin-http/reqwest breaks every plugin call, so hand it back to the
    // native webview fetch untouched.
    if (
      parsed.hostname === "ipc.localhost" ||
      parsed.protocol === "ipc:" ||
      parsed.protocol === "tauri:"
    ) {
      return browserFetch(input, init);
    }

    // The resolved absolute URL, not `input` — reqwest cannot resolve a relative one.
    return tauriFetch(parsed, init);
  };
}
