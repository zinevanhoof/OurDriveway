<script setup lang="ts">
import MobileNavbar from '@/components/MobileNavbar.vue';
import LocationMap from '@/components/map/LocationMap.vue';
import { useAuthStore } from '@/stores/auth';
import { computed } from 'vue';
import { RouterView, useRoute } from 'vue-router';

const auth = useAuthStore();

const route = useRoute()

// The map lives here rather than in its route's view, for two reasons that are really
// one: a MapLibre map is expensive to create and everything it has fetched — style,
// sprite, glyphs, tiles — dies with it. Mounted at the layout it is built the moment
// the app has a session, warms up around the user while the app sits on another
// screen, and is never torn down, so the Map tab opens on an already-drawn map.
//
// `invisible` rather than `v-if`/`v-show`: a `display: none` container measures 0×0,
// so MapLibre would size its canvas to nothing and fetch no tiles, which is the whole
// point of mounting it early. `visibility: hidden` keeps the box — and stops it
// hit-testing, so taps still land on the screen the user is actually looking at.
const onMap = computed(() => route.name === 'search')
</script>

<template>
    <main class="relative no-scrollbar flex flex-1 flex-col overflow-y-auto bg-background text-foreground">
        <RouterView />
        <!-- After the RouterView so it covers the (empty) SearchView on the map route.
             The wrapper owns the positioning: LocationMap's own root is `relative` for
             the controls it stacks, and merging `absolute` onto it leaves two position
             utilities of equal specificity fighting. -->
        <div v-if="auth.isAuthenticated" class="absolute inset-0" :class="{ invisible: !onMap }">
            <!-- Hasselt, Grote Markt — until the user's own location comes in. -->
            <LocationMap class="h-full w-full" :active="onMap" :lng="5.3378" :lat="50.9306" :zoom="13" />
        </div>
    </main>

    <MobileNavbar v-if="auth.isAuthenticated" />
</template>
