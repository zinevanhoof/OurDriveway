<script setup lang="ts">
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { onMounted, onBeforeUnmount, ref, computed, watch, h, render } from "vue";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import type { FeatureCollection } from "geojson";
import { pinsFromFeatures, type PinData } from "@/lib/mapPins";
import { useDebounceFn } from "@vueuse/core";
import { toast } from "vue-sonner";
import { fetchSpot, fetchSpotsNear, viewKeys } from "@/api/viewApi";
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import type { SpotFilter } from "@/types/SpotFilter";
import { spotMatches } from "@/lib/spotFilter";
import { mergeBooked } from "@/lib/bookingAvailability";
import { locateUser } from "@/lib/geo";
import MapPinComponent from "./MapPinComponent.vue";
import MapSearchComponent from "./MapSearchComponent.vue";
import SpotDetailDrawer from "../spot/SpotDetailDrawer.vue";
import BookingFormComponent from "../BookingFormComponent.vue";
import { Surface } from "@/components/base/surface";
import { Title } from "@/components/base/text";
import { Money } from "@/components/base/money";

// Dynamic OSM map via OpenFreeMap (Liberty vector style) + MapLibre GL. Keyless:
// tiles + style are fetched straight from the browser, no API key to expose.
const props = withDefaults(
  defineProps<{
    // Fallback center until we have the user's location.
    lng: number;
    lat: number;
    zoom?: number;
  }>(),
  { zoom: 14 },
);

const STYLE_URL = "https://tiles.openfreemap.org/styles/bright";
const el = ref<HTMLDivElement>();
let map: maplibregl.Map | undefined;

// Reactive query inputs, driven off the map viewport. Paused until the map's
// first `load`/`moveend` sets a center.
const center = ref<[number, number] | null>(null);
const meters = ref(0);
const filter = ref<SpotFilter>({ single: {} });

// Markers persist across selection so their transform transition can animate.
// Rebuild only when the fetched set changes; on select, re-render into the SAME
// divs (Vue patches in place) so the raised/shrink tween runs on live DOM nodes.
const selectedId = ref<string | null>(null);

const queryClient = useQueryClient();

// The caller's own spots are excluded server-side and unconditionally now — that used
// to be true of `SPOTS_NEARBY` and not of this one, so the map showed a host their own
// driveway as somewhere to park.
const { data: spotsInRadius } = useQuery({
  queryKey: computed(() =>
    viewKeys.nearby(center.value?.[0] ?? 0, center.value?.[1] ?? 0, meters.value),
  ),
  queryFn: () => fetchSpotsNear(center.value![0], center.value![1], meters.value),
  enabled: computed(() => center.value !== null),
});

// Full detail for the selected pin, fetched on click (disabled until then) so nothing
// runs at render time and there is one query total, not one per pin. The host's
// profile comes back on the same response — a LEFT JOIN now rather than a record link.
//
// `staleTime: 0` because this is the one read someone books against, and cached
// availability is stale by construction: a booking's status changes through events on
// the log, never through anything this client did, so there is nothing to invalidate
// on. The radius query keeps the default and re-runs on every pan.
//
// Freshness, not correctness. The authority is the server's availability check inside
// the reserve transaction; this only stops the picker offering slots it then retracts.
const { data: selectedSpot } = useQuery({
  queryKey: computed(() => viewKeys.spot(selectedId.value ?? "")),
  queryFn: () => fetchSpot(selectedId.value!),
  enabled: computed(() => selectedId.value !== null),
  staleTime: 0,
});

const reexecuteSpot = () =>
  queryClient.invalidateQueries({
    queryKey: viewKeys.spot(selectedId.value ?? ""),
  });

// The map filter stays client-side: which weekday and time slot a spot is open on is a
// fold over its availability, which a query cannot express. Recomputes on filter change
// without a refetch.
const matchedSpots = computed(() =>
  (spotsInRadius.value ?? []).filter((s) => spotMatches(s.availability, filter.value)),
);

// Refetch spots whenever the viewport settles. Radius = center → NE corner, so
// the circle covers the whole rectangular viewport (over-fetches a little).
function refreshBounds() {
  if (!map) return;
  const c = map.getCenter();
  center.value = [c.lng, c.lat];
  meters.value = c.distanceTo(map.getBounds().getNorthEast());
}
const onMoveEnd = useDebounceFn(refreshBounds, 1000);

// Drawer visibility is just "is a pin selected"; closing clears selection, which
// also animates the pin back down via the selectedId watch below.
const detailOpen = computed({
  get: () => selectedId.value !== null,
  set: (v) => { if (!v) selectedId.value = null; },
});

const bookingOpen = ref(false);

// The policy alone is not enough: `selectedId` is never cleared on close, so reopening
// the *same* pin changes neither variables nor pause state and urql does not re-execute —
// the picker would keep whatever it read the first time, including slots this renter has
// since held and abandoned. Opening the form is therefore an explicit refetch.
watch(bookingOpen, (isOpen) => {
  if (isOpen) void reexecuteSpot();
});

// ─── Clustering ────────────────────────────────────────────────────────────────
// Several spots can share one address (an apartment block's parking, a house with
// two driveways), so their markers land on the exact same pixel. MapLibre clusters
// GeoJSON *sources* natively — but only renders them through circle/symbol layers,
// which can't draw a Vue price bubble. So the source is used purely as a spatial
// index: MapLibre runs supercluster in its worker, we read the result back with
// querySourceFeatures and keep placing our own HTML markers.
const SOURCE_ID = "spots";

type ClusterSpot = { id: string; title: string; price: number };
// The open cluster's id is kept alongside its members so its pin can stay in the
// raised state while the drawer is up, exactly like a selected single spot.
const openCluster = ref<{ id: number; spots: ClusterSpot[] } | null>(null);

const clusterOpen = computed({
  get: () => openCluster.value !== null,
  set: (v) => { if (!v) openCluster.value = null; },
});

function toFeatureCollection(spots: any[]): FeatureCollection {
  return {
    type: "FeatureCollection",
    // Properties are what come back from getClusterLeaves, so everything the
    // cluster list renders has to live here — supercluster only keeps these.
    features: spots.map((s: any) => ({
      type: "Feature",
      // Two columns, not a geometry. `location { coordinates }` was a GeoJSON point
      // the database assembled; PostGIS is unavailable on YSQL, so lng/lat are stored
      // and returned separately — and GeoJSON wants them in exactly that order anyway.
      geometry: { type: "Point", coordinates: [s.lng, s.lat] },
      properties: { id: s.id, title: s.title, price: s.pricePerHour },
    })),
  };
}

type Pin = PinData & { div: HTMLDivElement; marker: maplibregl.Marker };
const pins = new Map<string, Pin>();

function renderPin(p: Pin) {
  const selected = p.clusterId !== null
    ? p.clusterId === openCluster.value?.id
    : p.spotId === selectedId.value;
  // Raise the marker element itself — MapLibre sets an inline z-index per marker
  // by latitude, so a z-class on the inner button can't lift it above siblings.
  p.div.style.zIndex = selected ? "10" : "";
  render(
    h(MapPinComponent, {
      pricePerHour: p.price,
      count: p.count,
      selected,
      onSelect: () => selectPin(p),
    }),
    p.div,
  );
}

// A cluster opens a list, it never zooms to expand. Spots at identical coordinates
// stay clustered at every zoom level, so zoom-to-expand would loop forever without
// ever reaching them — and that is exactly the case this whole feature exists for.
async function selectPin(p: Pin) {
  if (p.clusterId === null) {
    selectedId.value = p.spotId;
    return;
  }
  const source = map?.getSource(SOURCE_ID) as maplibregl.GeoJSONSource | undefined;
  if (!source) return;
  const leaves = await source.getClusterLeaves(p.clusterId, p.count, 0);
  openCluster.value = {
    id: p.clusterId,
    spots: leaves.map((f) => f.properties as ClusterSpot),
  };
}

function openSpot(id: string) {
  openCluster.value = null;
  selectedId.value = id;
}

// Rebuilds the marker set from whatever the source currently holds. Diffed by key
// rather than cleared and refilled: this runs on every `idle`, and recreating every
// marker each time flickers and restarts the selection transition.
function syncMarkers() {
  if (!map?.getSource(SOURCE_ID)) return;

  const next = pinsFromFeatures(map.querySourceFeatures(SOURCE_ID));

  for (const [key, pin] of pins) {
    if (next.has(key)) continue;
    pin.marker.remove();
    render(null, pin.div); // unmount before discarding the div (frees instances/effects)
    pins.delete(key);
  }
  for (const [key, d] of next) {
    const existing = pins.get(key);
    if (existing) {
      Object.assign(existing, d); // a cluster's count and cheapest price shift as it grows
      renderPin(existing);
      continue;
    }
    const div = document.createElement("div");
    const pin: Pin = {
      ...d,
      div,
      marker: new maplibregl.Marker({ element: div }).setLngLat(d.coords).addTo(map),
    };
    pins.set(key, pin);
    renderPin(pin);
  }
}

watch(matchedSpots, (spots) => {
  const source = map?.getSource(SOURCE_ID) as maplibregl.GeoJSONSource | undefined;
  source?.setData(toFeatureCollection(spots));
});

// Selection change: re-render each pin into its existing div → class flips,
// CSS transition animates the jump up and the shrink back. Closing either drawer
// clears its ref, which is what shrinks the pin again.
watch([selectedId, openCluster], () => {
  for (const p of pins.values()) renderPin(p);
});

onMounted(async () => {
  map = new maplibregl.Map({
    container: el.value!,
    style: STYLE_URL,
    center: [props.lng, props.lat],
    zoom: props.zoom,
    attributionControl: false,

  });
  // Liberty's vector tiles reference a few POI icons its sprite doesn't ship
  // (e.g. sports_centre). Feed a transparent 1×1 for those so MapLibre stops
  // warning — the POI just shows no icon, which is the case regardless.
  map.on("styleimagemissing", ({ id }) => {
    if (!map!.hasImage(id)) {
      map!.addImage(id, { width: 1, height: 1, data: new Uint8Array(4) });
    }
  });
  map.on("load", () => {
    map!.addSource(SOURCE_ID, {
      type: "geojson",
      data: toFeatureCollection(matchedSpots.value),
      cluster: true,
      clusterRadius: 50,
      // Cluster at every zoom the map allows. The default stops clustering a level
      // below the source maxzoom, and past that point spots sharing an address go
      // back to being separate markers stacked on one pixel — the bug this fixes.
      maxzoom: 22,
      clusterMaxZoom: 22,
    });
    // querySourceFeatures only sees *loaded* tiles, and MapLibre only loads tiles
    // for a source some layer actually draws. This layer exists solely to mark the
    // source as used — zero-radius circles render nothing.
    map!.addLayer({
      id: `${SOURCE_ID}-index`,
      type: "circle",
      source: SOURCE_ID,
      paint: { "circle-radius": 0 },
    });
    refreshBounds(); // initial fetch at the fallback center
  });
  map.on("moveend", onMoveEnd);
  // `idle` is the one event that guarantees tiles are loaded and clustering has
  // settled, which is exactly what querySourceFeatures needs.
  map.on("idle", syncMarkers);

  const here = await locateUser();
  if (here) {
    map.jumpTo({ center: here }); // fires moveend → refetch at the user
    const dot = document.createElement("div");
    dot.className = "size-4 rounded-full bg-blue-500 border-2 border-white shadow-md";
    new maplibregl.Marker({ element: dot }).setLngLat(here).addTo(map);
  } else {
    toast.error("Couldn't find your location", {
      description: "Check that location/GPS is on and permission is granted.",
      duration: 10000,
    });
  }
});

onBeforeUnmount(() => {
  for (const p of pins.values()) render(null, p.div);
  map?.remove();
});
</script>

<template>
  <div ref="el" class="relative h-full w-full">
    <MapSearchComponent @select="map?.jumpTo({ center: $event, zoom: 15 })" @filter="filter = $event" />
    <!-- Cluster tap: pick one of the spots sharing this location. -->
    <Drawer v-model:open="clusterOpen">
      <DrawerContent @close-auto-focus.prevent
        class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
        <div class="m-4 space-y-3">
          <Title size="lg">{{ openCluster?.spots.length }} spots here</Title>
          <div class="max-h-80 space-y-2 overflow-y-auto">
            <Surface v-for="s in openCluster?.spots" :key="s.id" @click="openSpot(s.id)" as="button" variant="none"
              orientation="horizontal" class="w-full justify-between gap-4 border border-border text-left">
              <Title as="span" weight="semibold" class="truncate">{{ s.title }}</Title>
              <Money :cents="s.price" suffix="/hr" size="md" weight="extrabold" tone="primary" class="shrink-0" />
            </Surface>
          </div>
        </div>
      </DrawerContent>
    </Drawer>
    <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" bookable
      @book="bookingOpen = true" />
    <BookingFormComponent v-model="bookingOpen" :spot="selectedSpot"
      :booked="mergeBooked(selectedSpot?.bookings)"
      @booked="() => reexecuteSpot()" />
  </div>
</template>
