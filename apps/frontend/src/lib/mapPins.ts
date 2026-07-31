// Turns MapLibre source features into the markers the map should show.
//
// Split out of LocationMap so it can be checked without a browser: the dedupe
// below is the kind of thing that fails silently — a wrong property name empties
// the map, a missed duplicate stacks two identical pins on one spot.

import type { Feature } from "geojson";

export type PinData = {
  key: string;
  coords: [number, number];
  /** Null for a cluster — its members can cost anything, so the pin shows a count. */
  price: number | null;
  count: number;
  spotId: string | null; // null for a cluster
  clusterId: number | null; // null for a single spot
};

/**
 * One entry per marker to draw, keyed so repeated calls can diff instead of
 * rebuilding.
 *
 * `querySourceFeatures` returns a feature once per tile it appears in, so a
 * cluster near a tile boundary comes back more than once. First one wins.
 */
export function pinsFromFeatures(features: Feature[]): Map<string, PinData> {
  const pins = new Map<string, PinData>();

  for (const f of features) {
    const p: any = f.properties ?? {};
    const key = p.cluster ? `c${p.cluster_id}` : `s${p.id}`;
    if (pins.has(key)) continue;

    pins.set(key, {
      key,
      coords: (f.geometry as any).coordinates as [number, number],
      price: p.cluster ? null : p.price,
      count: p.cluster ? p.point_count : 1,
      spotId: p.cluster ? null : p.id,
      clusterId: p.cluster ? p.cluster_id : null,
    });
  }

  return pins;
}

// ponytail: runnable self-check — call demo() from a scratch script (`npx tsx`)
// if you touch pinsFromFeatures().
export function demo() {
  const at = (props: any): Feature => ({
    type: "Feature",
    geometry: { type: "Point", coordinates: [5.26, 51.07] },
    properties: props,
  });
  const eq = (got: unknown, want: unknown, what: string) => {
    if (JSON.stringify(got) !== JSON.stringify(want))
      throw new Error(
        `${what}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`,
      );
  };

  eq(pinsFromFeatures([]).size, 0, "no features -> no pins");

  const single = pinsFromFeatures([at({ id: "spot:a", title: "A", price: 300 })]);
  eq(single.size, 1, "one spot -> one pin");
  eq(single.get("sspot:a")!.count, 1, "a single spot counts as one");
  eq(single.get("sspot:a")!.spotId, "spot:a", "single carries its spot id");
  eq(single.get("sspot:a")!.clusterId, null, "single has no cluster id");
  eq(single.get("sspot:a")!.price, 300, "single uses its own price");

  const cluster = pinsFromFeatures([
    at({ cluster: true, cluster_id: 7, point_count: 3 }),
  ]);
  eq(cluster.get("c7")!.count, 3, "cluster carries its size");
  eq(cluster.get("c7")!.price, null, "cluster shows no price");
  eq(cluster.get("c7")!.spotId, null, "a cluster is not one spot");
  eq(cluster.get("c7")!.clusterId, 7, "cluster carries its id for getClusterLeaves");

  // The tile-boundary case: same cluster returned twice, one marker expected.
  const dup = pinsFromFeatures([
    at({ cluster: true, cluster_id: 7, point_count: 3 }),
    at({ cluster: true, cluster_id: 7, point_count: 3 }),
    at({ id: "spot:a", title: "A", price: 300 }),
    at({ id: "spot:a", title: "A", price: 300 }),
  ]);
  eq(dup.size, 2, "duplicates across tiles collapse");

  // A cluster and a spot never collide even if the raw ids look alike.
  const mixed = pinsFromFeatures([
    at({ cluster: true, cluster_id: 1, point_count: 2 }),
    at({ id: "1", title: "One", price: 400 }),
  ]);
  eq(mixed.size, 2, "cluster ids and spot ids share no keyspace");

  return "mapPins: all checks passed";
}
