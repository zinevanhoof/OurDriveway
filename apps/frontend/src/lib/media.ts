/**
 * Turns a stored media key into a URL the browser can load.
 *
 * Events store bare keys (`spots/019f….jpeg`), never absolute URLs — an event
 * outlives whatever hostname was serving images the day it was written, and the
 * r2.dev development URL is rate-limited and explicitly not for production. The
 * hostname therefore lives in exactly one place per environment: VITE_MEDIA_BASE
 * in `.env.development` / `.env.production`, which vite picks by mode.
 */
const BASE = import.meta.env.VITE_MEDIA_BASE;

// Overloaded so a caller that already knows it has a key gets a plain `string`
// back: `AvatarImage`'s `src` is a required string, and every one of those call
// sites is behind a `v-if` on the same value.
export function imageUrl(key: string): string;
export function imageUrl(key: string | null | undefined): string | undefined;
export function imageUrl(key: string | null | undefined): string | undefined {
  // `undefined` rather than an empty string: an `<img src="">` re-requests the
  // current page, so callers must be able to `v-if` this away.
  if (!key) return undefined;
  // Blob URLs from a just-picked file are already loadable. Passing them through
  // is what lets a picker hold keys and pending Files in one list.
  if (key.startsWith("blob:") || key.startsWith("data:")) return key;
  return `${BASE}/${key}`;
}
