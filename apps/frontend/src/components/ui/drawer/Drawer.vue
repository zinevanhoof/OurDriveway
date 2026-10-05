<script lang="ts">
// When the last press began, for every drawer at once: one listener, not one per sheet.
let lastPressAt = 0
if (typeof document !== 'undefined') {
  document.addEventListener('pointerdown', () => { lastPressAt = performance.now() }, { capture: true, passive: true })
}
</script>

<script lang="ts" setup>
import type { DrawerRootEmits, DrawerRootProps } from 'vaul-vue'
import type { ComputedRef } from 'vue'
import { nextTick, watch } from 'vue'
import { useForwardPropsEmits } from 'reka-ui'
import { DrawerRoot } from 'vaul-vue'

const props = withDefaults(defineProps<DrawerRootProps>(), {
  shouldScaleBackground: true,
})

const emits = defineEmits<DrawerRootEmits>()

/**
 * Drops the close that the tap which *opened* the sheet would otherwise cause.
 *
 * A closed sheet stays mounted for its 500ms close animation, and for that long its
 * layer is still listening for a press "outside". Reopen it with a tap inside that
 * window and the same tap is that press: its `click` sets `open` to true, Vue flushes,
 * and a moment later, still inside the one click, the layer reports an outside press on
 * what is by then an open sheet and closes it. The sheet opens and shuts at once and the
 * tap looks ignored. A sheet opened from cold never sees this, because its layer did not
 * exist yet when the finger went down.
 *
 * So a close only counts if a press has begun since the sheet opened. The 100ms bound
 * keeps closes that involve no press at all (Escape) working; the echo arrives within
 * a few milliseconds of the open.
 */
let openedAt = 0
const filtered = ((event: string, ...args: unknown[]) => {
  if (event === 'update:open' && args[0] === false
    && lastPressAt <= openedAt && performance.now() - openedAt < 100) return
  ;(emits as (event: string, ...args: unknown[]) => void)(event, ...args)
}) as typeof emits

const forwarded = useForwardPropsEmits(props, filtered) as ComputedRef<Record<string, unknown>>

/**
 * Hands input back to the app the moment a sheet starts closing.
 *
 * A modal layer makes reka-ui set `pointer-events: none` on `<body>`, and it only
 * restores that when the layer *unmounts* — which vaul delays by its full 500ms close
 * animation. The overlay has finished fading out in a fraction of that, so the app looks
 * ready for half a second in which every tap is swallowed. This is one half of the gap;
 * the other is the overlay and the sheet themselves, still mounted on top, which
 * DrawerOverlay and DrawerContent each step out of the way of.
 *
 * Guarded on any overlay that is still open, so a sheet closing above another one leaves
 * the lock to the one underneath. Every `<Drawer>` in this app binds `:open`, so watching
 * the prop sees every close.
 */
watch(() => props.open, (open) => {
  if (open) {
    openedAt = performance.now()
    return
  }
  nextTick(() => {
    if (!document.querySelector('[data-vaul-overlay][data-state="open"]'))
      document.body.style.pointerEvents = ''
  })
})
</script>

<template>
  <DrawerRoot
    v-slot="slotProps"
    data-slot="drawer"
    v-bind="forwarded"
  >
    <slot v-bind="slotProps" />
  </DrawerRoot>
</template>
