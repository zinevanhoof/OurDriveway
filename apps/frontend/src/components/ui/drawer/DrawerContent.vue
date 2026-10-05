<script lang="ts" setup>
import type { DialogContentEmits, DialogContentProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { ref, watch } from 'vue'
import { injectDialogRootContext, useForwardPropsEmits } from 'reka-ui'
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

/**
 * Puts a sheet that is reopened mid-close back where it belongs.
 *
 * Dragging writes `transform: translate3d(0, <drag>px, 0)` and `transition: none` onto
 * the sheet as inline styles, and vaul leaves them there when the drag ends in a close:
 * the element is about to unmount anyway. Unless it is reopened inside the 500ms that
 * takes. Then the same element comes back, slides up, and settles on the leftover
 * transform, which is wherever the finger let go, usually most of the way off screen.
 * That is why a second pin tapped right after swiping a sheet away seemed to do nothing,
 * and why closing by tapping outside, which leaves no transform, never showed it.
 */
const content = ref<InstanceType<typeof DrawerContent> | null>(null)
const { open } = injectDialogRootContext()
watch(open, (isOpen) => {
  const el = content.value?.$el
  if (!isOpen || !(el instanceof HTMLElement)) return
  el.style.transform = ''
  el.style.transition = ''
})

/**
 * Stops Android from eating the next tap after the sheet is swiped away.
 *
 * Chromium sees a swipe that ends with any speed as the start of a fling, and for the
 * 500–700ms that fling would last it drops the `click` of the next tap, on the grounds
 * that the tap was only meant to stop the scrolling. There is nothing scrolling: vaul
 * moves the sheet itself. But the tap is gone all the same, `pointerdown` and
 * `pointerup` arrive and `click` never does, which is why swiping a sheet shut left the
 * app deaf for over half a second where tapping outside it did not. Measured on the
 * A70 with no drawer on the page at all, so it is the engine, not vaul.
 *
 * Cancelling the touchmove tells the engine the page handled the gesture, so it never
 * starts a fling. Only while vaul is dragging (it sets this class on the first move, and
 * the pointermove that does so is dispatched before this touchmove), because cancelling
 * unconditionally would also stop the content inside the sheet from scrolling. And only
 * for a move that is more down than across: vaul starts dragging on any downward drift,
 * and a sideways swipe on a photo strip has to stay the browser's to scroll.
 */
let startX = 0
let startY = 0
function trackStart(event: TouchEvent) {
  startX = event.touches[0].clientX
  startY = event.touches[0].clientY
}
function claimDrag(event: TouchEvent) {
  const el = content.value?.$el
  const touch = event.touches[0]
  if (!event.cancelable || !touch || !(el instanceof HTMLElement)) return
  if (Math.abs(touch.clientY - startY) <= Math.abs(touch.clientX - startX)) return
  if (el.classList.contains('vaul-dragging')) event.preventDefault()
}
</script>

<template>
  <DrawerPortal>
    <DrawerOverlay v-if="overlay" />
    <!-- `data-closed:pointer-events-none!`: the sheet is still mounted while it slides
         away and would take the taps meant for what it is uncovering. The bang because
         reka-ui sets `pointer-events: auto` inline, which a plain class cannot beat. -->
    <DrawerContent
      ref="content"
      data-slot="drawer-content"
      v-bind="{ ...$attrs, ...forwarded }"
      @touchstart.passive="trackStart"
      @touchmove="claimDrag"
      :class="cn(
        'bg-popover text-popover-foreground flex h-auto flex-col text-sm data-[vaul-drawer-direction=bottom]:inset-x-0 data-[vaul-drawer-direction=bottom]:bottom-0 data-[vaul-drawer-direction=bottom]:mt-24 data-[vaul-drawer-direction=bottom]:max-h-[80vh] data-[vaul-drawer-direction=bottom]:rounded-t-xl data-[vaul-drawer-direction=bottom]:border-t data-[vaul-drawer-direction=left]:inset-y-0 data-[vaul-drawer-direction=left]:left-0 data-[vaul-drawer-direction=left]:w-3/4 data-[vaul-drawer-direction=left]:rounded-r-xl data-[vaul-drawer-direction=left]:border-r data-[vaul-drawer-direction=right]:inset-y-0 data-[vaul-drawer-direction=right]:right-0 data-[vaul-drawer-direction=right]:w-3/4 data-[vaul-drawer-direction=right]:rounded-l-xl data-[vaul-drawer-direction=right]:border-l data-[vaul-drawer-direction=top]:inset-x-0 data-[vaul-drawer-direction=top]:top-0 data-[vaul-drawer-direction=top]:mb-24 data-[vaul-drawer-direction=top]:max-h-[80vh] data-[vaul-drawer-direction=top]:rounded-b-xl data-[vaul-drawer-direction=top]:border-b data-[vaul-drawer-direction=left]:sm:max-w-sm data-[vaul-drawer-direction=right]:sm:max-w-sm group/drawer-content fixed z-50 data-closed:pointer-events-none!',
        props.class,
      )"
    >
      <div class="bg-muted mt-4 h-1.5 w-[100px] rounded-full mx-auto hidden shrink-0 group-data-[vaul-drawer-direction=bottom]/drawer-content:block" />
      <slot />
    </DrawerContent>
  </DrawerPortal>
</template>
