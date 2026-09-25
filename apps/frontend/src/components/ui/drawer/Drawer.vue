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

const forwarded = useForwardPropsEmits(props, emits) as ComputedRef<Record<string, unknown>>

/**
 * Hands input back to the app the moment a sheet starts closing.
 *
 * A modal layer makes reka-ui set `pointer-events: none` on `<body>`, and it only
 * restores that when the layer *unmounts* — which vaul delays by its full 500ms close
 * animation. The overlay has finished fading out in a fraction of that, so the app looks
 * ready for half a second in which every tap is swallowed. This is the gap; the overlay's
 * own `data-closed:pointer-events-none` covers the element still sitting on top.
 *
 * Guarded on any overlay that is still open, so a sheet closing above another one leaves
 * the lock to the one underneath. Every `<Drawer>` in this app binds `:open`, so watching
 * the prop sees every close.
 */
watch(() => props.open, (open) => {
  if (open) return
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
