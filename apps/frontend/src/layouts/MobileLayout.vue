<script setup lang="ts">
import MobileNavbar from '@/components/layout/MobileNavbar.vue';
import LocationMap from '@/components/map/LocationMap.vue';
import { useAuthStore } from '@/stores/auth';
import { computed, ref, watch } from 'vue';
import { RouterView, useRoute } from 'vue-router';
import { AnimatePresence, motion } from 'motion-v';
import { COVERED, navDirection, pageVariants } from '@/router/transition';

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

// ─── the map across a page transition ───────────────────────────────────────
//
// The route flips on the frame the navigation commits; the page it belongs to is still
// sliding for another half second. Doing anything to the map on that frame is what made
// leaving the map snap: the map came back over a page that had not finished leaving.
//
// So: one flag for "a transition involving the map is in flight", and the two things that
// depend on it want opposite ends of it.
const settling = ref(false)
let timer: ReturnType<typeof setTimeout>

watch(onMap, () => {
    clearTimeout(timer)
    settling.value = true
    // A timer rather than `AnimatePresence`'s `exit-complete`, because a tab switch's exit
    // is instantaneous by design and would clear this before the arriving page had faded
    // up. Nothing here needs the moment exactly: by then the map is behind an opaque page,
    // so being late costs nothing and being early is the bug.
    timer = setTimeout(() => (settling.value = false), COVERED * 1000)
})

// Shown early, hidden late: on the way back it has to be there *under* the page still
// sliding off, so that page reveals a map rather than uncovering a blank box the map then
// appears in.
const mapShown = computed(() => onMap.value || settling.value)

// `active` is the mirror image — false early, true late. It gates the two sheets inside
// LocationMap, which portal to the body and so escape the page stack entirely: unmounting
// them has to happen the frame the route changes (or they hang over the next screen), and
// re-mounting has to wait for the transition (or the sheet you left slides up over a page
// still on its way out).
const mapActive = computed(() => onMap.value && !settling.value)
</script>

<template>
    <!-- `overflow-hidden`, where this used to scroll: the arriving page starts a full
         width off to one side, and anything but a clip here turns that into a horizontal
         scrollbar. The scrolling moved one level in, onto the page box below. -->
    <main class="relative flex flex-1 flex-col overflow-hidden bg-background text-foreground">
        <!-- One page slides over another, so two are mounted at once and each needs to be
             taken out of the flow and given the whole box — hence `absolute inset-0` per
             page rather than a single positioned parent. That also makes each page the
             containing block for its own `DetailLayout` action bar, which is the point:
             the bar travels with the screen it belongs to instead of both screens' bars
             stacking on the layout.

             `flex flex-col` and `overflow-y-auto` are what `<main>` used to carry. A page
             that brings its own scroll container (every `DetailLayout` and `TabLayout`
             screen) never overflows this box, which is what keeps `bottom-0` on the action
             bar pinned; a page without one (checkout, the auth screens) scrolls here, as
             it did before.

             **`bg-background` per page, not only on `<main>`.** A page whose own markup
             does not paint every pixel — the header strip above a `TabLayout`, the gaps
             between cards — is otherwise transparent, and a transparent page sliding over
             another shows it through. Being on top is not the same as being opaque.

             The map route is the exception, and the reason is the layer below: its view is
             an empty `<div />` and the map shows *through* it, so it must not paint a
             background and must not swallow the taps meant for the map.

             `:initial="false"` so a cold load paints the first screen rather than sliding
             it in from nowhere. -->
        <RouterView v-slot="{ Component, route: page }">
            <AnimatePresence :initial="false" :custom="navDirection">
                <motion.div :key="page.path" :custom="navDirection" :variants="pageVariants" initial="enter"
                    animate="center" exit="leave" class="absolute inset-0 no-scrollbar flex flex-col overflow-y-auto"
                    :class="page.name === 'search' ? 'pointer-events-none' : 'bg-background'">
                    <component :is="Component" />
                </motion.div>
            </AnimatePresence>
        </RouterView>
        <!-- **Under the pages, not over them.** It used to sit on top, which worked while a
             page swap was instant and broke the moment one could slide: a page leaving the
             map had the map painted over it for the whole animation. Below it instead, at
             `z-0` against the pages' own `zIndex`, so a page slides over the map on the way
             in and uncovers it on the way out — and the map route's page box above is
             transparent, which is what lets it be seen at all.

             The wrapper owns the positioning: LocationMap's own root is `relative` for
             the controls it stacks, and merging `absolute` onto it leaves two position
             utilities of equal specificity fighting. -->
        <div v-if="auth.isAuthenticated" class="absolute inset-0 z-0" :class="{ invisible: !mapShown }">
            <!-- Hasselt, Grote Markt — until the user's own location comes in. -->
            <LocationMap class="h-full w-full" :active="mapActive" :lng="5.3378" :lat="50.9306" :zoom="13" />
        </div>
    </main>

    <MobileNavbar v-if="auth.isAuthenticated" />
</template>
