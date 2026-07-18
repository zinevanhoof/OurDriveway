<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, computed, watch, h, render } from "vue";
import { MapPin } from "@lucide/vue";
import maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { useDebounceFn } from "@vueuse/core";
import { toast } from "vue-sonner";
import { useServiceQuery } from "@/composables/useServiceQuery";
import { SPOTS_IN_RADIUS } from "@/api/graphql/spot";
import { native } from "@/api/http";

// Dynamic OSM map via LocationIQ + MapLibre GL. The map key is a separate,
// domain/rate-restricted LocationIQ key shipped client-side (MapLibre fetches
// styles + tiles straight from the browser) — NOT the secret autocomplete key,
// which stays server-side in spot-service.
const props = withDefaults(
  defineProps<{
    // Fallback center until we have the user's location.
    lng: number;
    lat: number;
    zoom?: number;
    // "vector" (GPU, crisp at any zoom) or "raster" (plain tiles). Both from LocationIQ.
    kind?: "vector" | "raster";
    // LocationIQ style name: streets, dark, light, ...
    style?: string;
  }>(),
  { zoom: 14, kind: "vector", style: "streets" },
);

const key = import.meta.env.VITE_LOCATIONIQ_MAP_KEY;
const el = ref<HTMLDivElement>();
let map: maplibregl.Map | undefined;

function styleUrl() {
  const path = props.kind === "vector" ? "vector.json" : "r/style.json";
  return `https://tiles.locationiq.com/v3/${props.style}/${path}?key=${key}`;
}

// Reactive query inputs, driven off the map viewport. Paused until the map's
// first `load`/`moveend` sets a center.
const center = ref<[number, number] | null>(null);
const meters = ref(0);

const { data } = useServiceQuery("spot", {
  query: SPOTS_IN_RADIUS,
  variables: computed(() => ({
    lng: center.value?.[0],
    lat: center.value?.[1],
    meters: meters.value,
  })),
  pause: computed(() => center.value === null),
});

// Refetch spots whenever the viewport settles. Radius = center → NE corner, so
// the circle covers the whole rectangular viewport (over-fetches a little).
function refreshBounds() {
  if (!map) return;
  const c = map.getCenter();
  center.value = [c.lng, c.lat];
  meters.value = c.distanceTo(map.getBounds().getNorthEast());
}
const onMoveEnd = useDebounceFn(refreshBounds, 250);

// Cheap and readable: drop every marker and re-add the fetched set each time.
let markers: maplibregl.Marker[] = [];
watch(
  () => data.value?.spots,
  (spots) => {
    if (!map) return;
    for (const m of markers) m.remove();
    markers = (spots ?? []).map((s: any) => {
      const div = document.createElement("div");
      render(h(MapPin, { size: 32 }), div);
      return new maplibregl.Marker({ element: div })
        .setLngLat(s.location.coordinates as [number, number])
        .addTo(map!);
    });
  },
);

// One-shot user position for the initial center. On native, run the permission
// flow (browser geolocation is dead in mobile webviews); on web, ask the browser.
async function locateUser(): Promise<[number, number] | null> {
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

onMounted(async () => {
  if (!key) {
    console.error("VITE_LOCATIONIQ_MAP_KEY is not set");
    return;
  }
  map = new maplibregl.Map({
    container: el.value!,
    style: styleUrl(),
    center: [props.lng, props.lat],
    zoom: props.zoom,
  });
  map.on("load", refreshBounds); // initial fetch at the fallback center
  map.on("moveend", onMoveEnd);

  const here = await locateUser();
  if (here) {
    map.jumpTo({ center: here }); // fires moveend → refetch at the user
  } else {
    toast.error("Couldn't find your location", {
      description: "Check that location/GPS is on and permission is granted.",
      duration: 10000,
    });
  }
});

onBeforeUnmount(() => map?.remove());
</script>

<template>
  <div ref="el" class="h-full w-full" />
</template>
