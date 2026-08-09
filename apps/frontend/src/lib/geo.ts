// Where the user is, and which of two points is closer. Shared by the map and the
// home screen's "spots near you" — the map owns neither.

import { native } from "@/api/http";

/** A `[lng, lat]` pair, in that order — GeoJSON's, MapLibre's, and the view's. */
export type Position = [number, number];

/**
 * One-shot user position, or null if we can't have it.
 *
 * The webview's geolocation works on both mobile and web, but on native we prefer
 * the platform's native geolocation (via the permission flow) since it's much more
 * accurate.
 *
 * Never throws and never toasts: a caller that can carry on without a position
 * (the map has a fallback center) shouldn't have to catch, and one that can't
 * decides for itself what to say.
 */
export async function locateUser(): Promise<Position | null> {
  try {
    if (native) {
      const geo = await import("@tauri-apps/plugin-geolocation");
      let perms = await geo.checkPermissions();
      if (perms.location === "prompt" || perms.location === "prompt-with-rationale") {
        perms = await geo.requestPermissions(["location"]);
      }
      if (perms.location !== "granted") return null;
      const pos = await geo.getCurrentPosition();
      return [pos.coords.longitude, pos.coords.latitude];
    }
    if (!navigator.geolocation) return null;
    return await new Promise((resolve) => {
      navigator.geolocation.getCurrentPosition(
        (p) => resolve([p.coords.longitude, p.coords.latitude]),
        () => resolve(null),
      );
    });
  } catch {
    return null;
  }
}

/**
 * A sort key for "which of these is nearer to me", in no unit at all.
 *
 * Longitude degrees are scaled by cos(latitude) so they carry the same weight as
 * latitude degrees; the result is the squared distance in those scaled degrees.
 * Monotonic with the real distance, which is all an ordering needs.
 *
 * ponytail: flat-earth approximation, and squared. Fine over a few km inside one
 * radius query, wrong near the poles and across the antimeridian, and it is NOT a
 * distance — never print it. Swap in haversine the day a card shows "1.2 km away".
 */
export function nearer(from: Position, to: Position): number {
  const [lng, lat] = from;
  const scale = Math.cos((lat * Math.PI) / 180);
  return ((to[0] - lng) * scale) ** 2 + (to[1] - lat) ** 2;
}
