<script lang="ts" setup>
import type { DialogContentEmits, DialogContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { useForwardPropsEmits } from 'reka-ui'
import { reactiveOmit } from '@vueuse/core'
import { DrawerContent, DrawerPortal } from 'vaul-vue'
import { cn } from '@/lib/utils'
import DrawerOverlay from './DrawerOverlay.vue'

defineOptions({
  inheritAttrs: false,
})

const props = withDefaults(
  defineProps<DialogContentProps & {
    class?: HTMLAttributes['class']
    /**
     * The dimming layer behind the sheet. Off for a non-modal drawer: it is
     * `fixed inset-0`, so it swallows every tap on whatever is behind it and stays
     * mounted for the full close animation, which defeats the point of going
     * non-modal in the first place.
     */
    overlay?: boolean
  }>(),
  { overlay: true },
)
const emits = defineEmits<DialogContentEmits>()

// `overlay` is ours, not vaul's — forwarding it would land it on the DOM node.
const forwarded = useForwardPropsEmits(reactiveOmit(props, 'overlay'), emits)
</script>

<template>
  <DrawerPortal>
    <DrawerOverlay v-if="overlay" />
    <DrawerContent
      data-slot="drawer-content"
      v-bind="{ ...$attrs, ...forwarded }"
      :class="cn(
        'bg-popover text-popover-foreground flex h-auto flex-col text-sm data-[vaul-drawer-direction=bottom]:inset-x-0 data-[vaul-drawer-direction=bottom]:bottom-0 data-[vaul-drawer-direction=bottom]:mt-24 data-[vaul-drawer-direction=bottom]:max-h-[80vh] data-[vaul-drawer-direction=bottom]:rounded-t-xl data-[vaul-drawer-direction=bottom]:border-t data-[vaul-drawer-direction=left]:inset-y-0 data-[vaul-drawer-direction=left]:left-0 data-[vaul-drawer-direction=left]:w-3/4 data-[vaul-drawer-direction=left]:rounded-r-xl data-[vaul-drawer-direction=left]:border-r data-[vaul-drawer-direction=right]:inset-y-0 data-[vaul-drawer-direction=right]:right-0 data-[vaul-drawer-direction=right]:w-3/4 data-[vaul-drawer-direction=right]:rounded-l-xl data-[vaul-drawer-direction=right]:border-l data-[vaul-drawer-direction=top]:inset-x-0 data-[vaul-drawer-direction=top]:top-0 data-[vaul-drawer-direction=top]:mb-24 data-[vaul-drawer-direction=top]:max-h-[80vh] data-[vaul-drawer-direction=top]:rounded-b-xl data-[vaul-drawer-direction=top]:border-b data-[vaul-drawer-direction=left]:sm:max-w-sm data-[vaul-drawer-direction=right]:sm:max-w-sm group/drawer-content fixed z-50',
        props.class,
      )"
    >
      <div class="bg-muted mt-4 h-1.5 w-[100px] rounded-full mx-auto hidden shrink-0 group-data-[vaul-drawer-direction=bottom]/drawer-content:block" />
      <slot />
    </DrawerContent>
  </DrawerPortal>
</template>
