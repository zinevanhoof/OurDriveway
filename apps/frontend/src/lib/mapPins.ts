// Groups the map's spots into the markers to draw: one per spot, or one per group of
// spots too close together to tell apart at the current zoom.
//
// Computed here, in plain arithmetic, rather than by a clustered MapLibre GeoJSON
// source. That source clusters inside its tiles, and reading the result back with
// `querySourceFeatures` returns every tile that happens to be loaded — during a zoom,
// the old level's and the new level's at once — so a "2 spots" pin and the two spots it
// stood for were drawn side by side. The only event that waited that out was `idle`,
// which also waits for the entire basemap. Computed directly, the answer is exact the
// moment the zoom level or the data changes.
//
// Kept free of MapLibre so the self-check below runs under plain `npx tsx`.

export type MapSpot = {
  id: string;
  title: string;
  /** EUR cents per hour. */
  price: number;
  lng: number;
  lat: number;
};

export type PinData = {
  key: string;
  coords: [number, number];
  /** Null for a group — its members can cost anything, so the pin shows a count. */
  price: number | null;
  /** Every spot under this pin. One for a single spot. */
  spots: MapSpot[];
};

/** How close, in screen pixels, two spots may be before they share a pin. */
export const RADIUS_PX = 50;

/** Web Mercator, in pixels of a world `512 × 2^zoom` wide — MapLibre's own tile size. */
function toPixels(lng: number, lat: number, zoom: number): [number, number] {
  const scale = 512 * 2 ** zoom;
  const sin = Math.sin((lat * Math.PI) / 180);
  return [
    ((lng + 180) / 360) * scale,
    (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * scale,
  ];
}

/**
 * The pins for `spots` at `zoom`.
 *
 * Greedy: walk the spots in id order, and each one not yet taken gathers every untaken
 * spot within {@link RADIUS_PX} of it. Worked out per *whole* zoom level rather than per
 * frame, so panning never reshuffles a group — only crossing a zoom level does. Spots at
 * one address are 0 px apart and share a pin at every zoom, which is the case the
 * cluster drawer exists for: zooming in would never separate them.
 *
 * ponytail: O(n²) per zoom level — nothing for the few hundred spots one radius query
 * returns. A grid index, or grouping on the backend (see the README's to-do list), if
 * that ever grows.
 */
export function clusterPins(spots: MapSpot[], zoom: number): Map<string, PinData> {
  const level = Math.floor(zoom);
  // Sorted so the same spots always form the same groups, whatever order they arrived in.
  const sorted = [...spots].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const px = sorted.map((s) => toPixels(s.lng, s.lat, level));
  const taken = new Array<boolean>(sorted.length).fill(false);
  const pins = new Map<string, PinData>();

  for (let i = 0; i < sorted.length; i++) {
    if (taken[i]) continue;

    const group = [sorted[i]];
    for (let j = i + 1; j < sorted.length; j++) {
      const [dx, dy] = [px[j][0] - px[i][0], px[j][1] - px[i][1]];
      if (!taken[j] && Math.hypot(dx, dy) <= RADIUS_PX) {
        taken[j] = true;
        group.push(sorted[j]);
      }
    }

    // Keyed by who is in it, so a pin that survives a refetch keeps its marker. The two
    // prefixes keep a group's key and a spot's key from ever colliding.
    const key = group.length === 1 ? `s${group[0].id}` : `c${group.map((s) => s.id).join(",")}`;
    const mean = (pick: (s: MapSpot) => number) =>
      group.reduce((sum, s) => sum + pick(s), 0) / group.length;

    pins.set(key, {
      key,
      coords: [mean((s) => s.lng), mean((s) => s.lat)],
      price: group.length === 1 ? group[0].price : null,
      spots: group,
    });
  }

  return pins;
}

// ponytail: runnable self-check — call demo() from a scratch script (`npx tsx`)
// if you touch clusterPins().
export function demo() {
  const eq = (got: unknown, want: unknown, what: string) => {
    if (JSON.stringify(got) !== JSON.stringify(want))
      throw new Error(`${what}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`);
  };
  const spot = (id: string, lat: number, lng: number): MapSpot => ({
    id, title: id, price: 300, lng, lat,
  });

  // Two driveways ~145 m apart in Hasselt, one far away in Genk, and two at one address.
  const havermarkt = spot("a", 50.9299, 5.3368);
  const zuivelmarkt = spot("b", 50.9312, 5.3372);
  const genk = spot("c", 50.9650, 5.5000);
  const flat1 = spot("d", 50.9400, 5.3300);
  const flat2 = spot("e", 50.9400, 5.3300);

  eq(clusterPins([], 14).size, 0, "no spots -> no pins");

  const one = clusterPins([havermarkt], 14).get("sa")!;
  eq([one.price, one.spots.length], [300, 1], "a lone spot is its own pin, with its price");

  const zoomedOut = clusterPins([havermarkt, zuivelmarkt, genk], 12);
  eq(zoomedOut.size, 2, "at zoom 12 the two Hasselt spots share a pin, Genk stands alone");
  eq(zoomedOut.get("ca,b")?.spots.length, 2, "the shared pin holds both");
  eq(zoomedOut.get("ca,b")?.price, null, "a group shows a count, not a price");

  eq(clusterPins([havermarkt, zuivelmarkt, genk], 18).size, 3, "at zoom 18 all three separate");

  // The whole point of grouping at all: same address, together at every zoom.
  eq(clusterPins([flat1, flat2], 22).size, 1, "one address stays one pin at max zoom");

  // Fractional zooms group like their whole level, so a pan never reshuffles.
  eq(
    [...clusterPins([havermarkt, zuivelmarkt], 12.9).keys()],
    [...clusterPins([havermarkt, zuivelmarkt], 12).keys()],
    "12.9 groups exactly like 12",
  );

  // Order-independent, which is what lets a refetch keep the same markers.
  eq(
    [...clusterPins([zuivelmarkt, genk, havermarkt], 12).keys()],
    [...clusterPins([havermarkt, zuivelmarkt, genk], 12).keys()],
    "input order does not change the groups",
  );

  return "mapPins: all checks passed";
}
