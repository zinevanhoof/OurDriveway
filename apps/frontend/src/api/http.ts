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
// the whole app. It also picks the shell in `main.ts`: native renders the app directly,
// never inside WebLayout's phone frame.
//
// Its other callers are capability decisions too: the platform geolocation prompt
// (lib/geo.ts), opening a maps app (RenterBookingRow), and listening for App Links.
export const native = isTauri();

// reqwest can't resolve a relative URL and rejects the `tauri://` scheme a bundled
// build loads from, so native needs an absolute base: the public origin in a release
// build (`VITE_API_BASE` in .env.production — the same site as the App Link host in
// tauri.conf.json, which is where `catch_deep_link` gets it from).
//
// Under `tauri dev` it is the Vite server (.env.development), whose proxy
// (vite.config.ts) already reaches every service. Written out rather than taken from
// `window.location`: the app's page is `http://tauri.localhost` even in dev — Tauri
// fetches the dev server on the webview's behalf — and reqwest cannot resolve that.
// Still through plugin-http, so dev keeps exercising the release path; the address has
// to be in the http scope, which is what src-tauri/capabilities/dev-lan.json is for.
const apiBase: string | undefined = import.meta.env.VITE_API_BASE;

/**
 * The website's origin: the page's own on the web and in dev, `VITE_API_BASE`'s in the
 * released app, whose own origin (`http://tauri.localhost`) nothing outside the phone
 * can reach. For URLs handed to the outside world that must come back to the right
 * screen — a redirect payment's `return_url`.
 *
 * In dev on the phone that is `http://tauri.localhost` too, which only a return that
 * stays inside the webview can reach.
 */
export function webOrigin(): string {
  return native && apiBase && !import.meta.env.DEV
    ? new URL(apiBase).origin
    : window.location.origin;
}

// Replace window.fetch once, at startup, so every caller (king.ts REST + urql) stays
// on plain relative-URL fetch and is routed correctly without knowing about Tauri.
// Web is left untouched: relative URLs resolve same-origin against Caddy.
export function installNativeFetch() {
  if (!native) return;
  // Loud at startup rather than as every request failing to resolve later.
  if (!apiBase) throw new Error("VITE_API_BASE is not set for this build mode");

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
