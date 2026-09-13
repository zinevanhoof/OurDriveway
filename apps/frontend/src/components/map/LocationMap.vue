<script setup lang="ts">
import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/vue-query";
import { onMounted, onBeforeUnmount, ref, computed, watch, h, render } from "vue";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { clusterPins, type MapSpot, type PinData } from "@/lib/mapPins";
import { toast } from "vue-sonner";
import { fetchSpot, fetchSpotsNear } from "@/api/viewApi";
import { viewKeys } from "@/api/keys";
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
  // Every pan is a new key, and a new key starts out with no data — which emptied the
  // source and blinked every pin off until the response landed. Holding the previous
  // result until then means the pins are replaced in one step, never removed first;
  // `syncMarkers` diffs by key, so the ones still in view don't even re-mount.
  placeholderData: keepPreviousData,
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
//
// Straight off `moveend`, no debounce: it fires once, after a drag's inertia has run
// out, so there is no burst to smooth — a delay only left the old area on screen.
function refreshBounds() {
  if (!map) return;
  const c = map.getCenter();
  center.value = [c.lng, c.lat];
  meters.value = c.distanceTo(map.getBounds().getNorthEast());
}

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
// two driveways), so their markers land on the exact same pixel. Nearby spots are
// grouped into one pin by `clusterPins`, computed here rather than by a clustered
// MapLibre source — see `lib/mapPins.ts` for why reading clusters back out of tiles
// drew a group and its members at the same time.

// The open group's key is kept alongside its members so its pin can stay in the
// raised state while the drawer is up, exactly like a selected single spot.
const openCluster = ref<{ key: string; spots: MapSpot[] } | null>(null);

// Open state is its own ref rather than derived from `openCluster`, because the list is
// rendered *from* `openCluster`: clearing it the moment the drawer started closing emptied
// the sheet and collapsed its height mid-animation. vaul translates the sheet by its own
// height, so the close visibly jumped forward the instant the rows vanished. The members
// are held until vaul reports the animation finished — which also keeps the pin raised
// until the sheet is actually gone, rather than dropping it on the first frame.
const clusterOpen = ref(false);

// Two columns, not a geometry: PostGIS is unavailable on YSQL, so lng/lat come back
// separately.
const mapSpots = computed<MapSpot[]>(() =>
  matchedSpots.value.map((s) => ({
    id: s.id,
    title: s.title,
    price: s.pricePerHour,
    lng: s.lng,
    lat: s.lat,
  })),
);

// `div` is MapLibre's to position; `inner` is ours to animate. They have to be two
// elements: the individual `scale` property is applied *after* the element's own
// `transform`, so scaling the marker element multiplies the positioning translate with
// it and the pin slides off toward the transform origin as it shrinks.
type Pin = PinData & {
  div: HTMLDivElement;
  inner: HTMLDivElement;
  marker: maplibregl.Marker;
};
const pins = new Map<string, Pin>();

function renderPin(p: Pin) {
  // `clusterOpen`, not just `openCluster`: the members are deliberately held past the
  // start of the close so the sheet keeps its height (see the note on `clusterOpen`),
  // but the pin should start dropping the moment the drawer does.
  const selected = p.spots.length > 1
    ? p.key === openCluster.value?.key && clusterOpen.value
    : p.spots[0].id === selectedId.value;
  // Raise the marker element itself — MapLibre sets an inline z-index per marker
  // by latitude, so a z-class on the inner button can't lift it above siblings.
  p.div.style.zIndex = selected ? "10" : "";
  render(
    h(MapPinComponent, {
      pricePerHour: p.price,
      count: p.spots.length,
      selected,
      onSelect: () => selectPin(p),
    }),
    p.inner,
  );
}

// A cluster opens a list, it never zooms to expand. Spots at identical coordinates
// stay clustered at every zoom level, so zoom-to-expand would loop forever without
// ever reaching them — and that is exactly the case this whole feature exists for.
// Both sheets are non-modal, so either can be open when a pin is tapped — each one
// closes the other rather than stacking.
function selectPin(p: Pin) {
  if (p.spots.length === 1) {
    clusterOpen.value = false;
    selectedId.value = p.spots[0].id;
    return;
  }
  selectedId.value = null;
  openCluster.value = { key: p.key, spots: p.spots };
  clusterOpen.value = true;
}

/**
 * Drops a closed cluster's members, once it is safe to.
 *
 * vaul's `animationEnd` is not an animation event: it is a 500ms `setTimeout` started
 * when `open` changed, carrying the value `open` had at that moment, and it is never
 * cancelled. Closing one cluster and opening another inside that window delivers a
 * stale `false` after the new sheet is already up — which emptied it to its title.
 * So the live state decides, not the argument.
 */
function releaseCluster(open: boolean) {
  if (!open && !clusterOpen.value) openCluster.value = null;
}

function openSpot(id: string) {
  clusterOpen.value = false; // members stay until the close animation ends
  selectedId.value = id;
}

// Rebuilds the marker set for the current spots and zoom level. Diffed by key rather
// than cleared and refilled: recreating every marker flickers and restarts the
// selection transition, and a pin whose members did not change keeps its marker.
//
// Nothing here runs while panning — MapLibre moves the markers itself. Only new data
// or crossing a whole zoom level changes the groups, and both land here straight away,
// mid-gesture included.
let zoomLevel: number | null = null;

// Pins grow in and shrink out rather than blinking, so a refetch or a zoom step reads
// as the set changing instead of the map flickering.
const PIN_FADE_MS = 180;

// Applied to the wrapper inside the marker, never to the marker itself — see the note
// on `Pin`. The pin's own selection `scale-125` lives one level further in, on the
// button, so that transition and this one stay independent.
function fadePin(inner: HTMLDivElement, visible: boolean) {
  inner.style.transition = `opacity ${PIN_FADE_MS}ms ease, scale ${PIN_FADE_MS}ms ease`;
  inner.style.opacity = visible ? "1" : "0";
  inner.style.scale = visible ? "1" : "0.6";
}

function syncMarkers() {
  if (!map) return;
  zoomLevel = Math.floor(map.getZoom());

  const next = clusterPins(mapSpots.value, zoomLevel);

  for (const [key, pin] of pins) {
    if (next.has(key)) continue;
    // Dropped from the index first, so a key that comes back inside the fade gets a
    // fresh marker rather than adopting one that is on its way out.
    pins.delete(key);
    fadePin(pin.inner, false);
    setTimeout(() => {
      pin.marker.remove();
      render(null, pin.inner); // unmount before discarding it (frees instances/effects)
    }, PIN_FADE_MS);
  }
  for (const [key, d] of next) {
    const existing = pins.get(key);
    if (existing) {
      Object.assign(existing, d); // a refetched spot may have a new price or title
      renderPin(existing);
      continue;
    }
    const div = document.createElement("div");
    const inner = document.createElement("div");
    div.appendChild(inner);
    fadePin(inner, false);
    const pin: Pin = {
      ...d,
      div,
      inner,
      marker: new maplibregl.Marker({ element: div }).setLngLat(d.coords).addTo(map),
    };
    pins.set(key, pin);
    renderPin(pin);
    // The marker is in the DOM by now; reading a layout property commits the hidden
    // state so the flip below has something to transition *from*. Without it both
    // styles land in the same frame and the pin just appears.
    void inner.offsetWidth;
    fadePin(inner, true);
  }
}

watch(mapSpots, syncMarkers);

// Selection change: re-render each pin into its existing div → class flips,
// CSS transition animates the jump up and the shrink back. A single spot shrinks when
// `selectedId` clears; a cluster when `clusterOpen` flips, which is the first frame of
// the close rather than the last — `openCluster` outlives both by design.
watch([selectedId, openCluster, clusterOpen], () => {
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
    refreshBounds(); // initial fetch at the fallback center
  });
  // Clears the selection, which closes the detail sheet. That sheet is non-modal, so
  // nothing dismisses it for us any more. Bound on the canvas rather than via
  // `map.on("click")` because markers are sibling DOM elements of the canvas, not
  // children — a tap on a pin can never reach this, so selecting one cannot also
  // cancel it.
  map.getCanvas().addEventListener("click", () => {
    selectedId.value = null;
    clusterOpen.value = false;
  });
  map.on("moveend", refreshBounds);
  // Regroup the moment a zoom gesture crosses a whole level, not when it ends. `zoom`
  // fires every frame of a zoom, so this compares levels and only regroups on a change.
  map.on("zoom", () => {
    if (Math.floor(map!.getZoom()) !== zoomLevel) syncMarkers();
  });

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
  for (const p of pins.values()) render(null, p.inner);
  map?.remove();
});
</script>

<template>
  <div ref="el" class="relative h-full w-full">
    <MapSearchComponent @select="map?.jumpTo({ center: $event, zoom: 15 })" @filter="filter = $event" />
    <!-- Cluster tap: pick one of the spots sharing this location. -->
    <Drawer v-model:open="clusterOpen" :modal="false"
      @animation-end="releaseCluster">
      <!-- The list scrolls by scrolling the sheet itself, not an inner box. vaul only
           lets a downward drag close the drawer when the scroll container it finds is
           the dialog (it checks `role="dialog"` explicitly); from a nested scroller the
           gesture goes to that scroller and the sheet stays put. -->
      <!-- No overlay, and outside pointer-downs are left alone: that event beats the
           marker's click, so letting it dismiss would close and reopen the sheet on
           every pin-to-pin tap. The canvas click handler closes it instead. -->
      <DrawerContent @close-auto-focus.prevent :overlay="false" @pointer-down-outside.prevent
        class="overflow-y-auto data-[vaul-drawer-direction=bottom]:mb-15">
        <div class="m-4 space-y-3">
          <Title size="lg">{{ openCluster?.spots.length }} spots here</Title>
          <div class="space-y-2">
            <Surface v-for="s in openCluster?.spots" :key="s.id" @click="openSpot(s.id)" as="button" variant="none"
              orientation="horizontal" class="w-full justify-between gap-4 border border-border text-left">
              <Title as="span" weight="semibold" class="truncate">{{ s.title }}</Title>
              <Money :cents="s.price" suffix="/hr" size="md" weight="extrabold" tone="primary" class="shrink-0" />
            </Surface>
          </div>
        </div>
      </DrawerContent>
    </Drawer>
    <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" bookable :modal="false"
      @book="bookingOpen = true" />
    <BookingFormComponent v-model="bookingOpen" :spot="selectedSpot"
      :booked="mergeBooked(selectedSpot?.bookings)"
      @booked="() => reexecuteSpot()" />
  </div>
</template>
