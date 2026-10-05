<script lang="ts" setup>
import type { DialogOverlayProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import { onBeforeUnmount, ref, watch } from 'vue'
import { injectDialogRootContext } from 'reka-ui'
import { reactiveOmit } from '@vueuse/core'
import { DrawerOverlay } from 'vaul-vue'
import { cn } from '@/lib/utils'

const props = defineProps<DialogOverlayProps & { class?: HTMLAttributes['class'] }>()

const delegatedProps = reactiveOmit(props, 'class')

/**
 * Lets taps through the overlay while it fades out, a beat after the close.
 *
 * It stays mounted, covering the screen, for the 500ms of its fade, and would swallow
 * every tap for that long. But it cannot step aside on the very frame the sheet closes:
 * the tap that dismissed the sheet has not delivered its `click` yet, and with nothing
 * left in the way that click lands on whatever is underneath, so dismissing over a
 * button would press it. 80ms is past that click and well short of a second tap. Only
 * a close that came from a press on the overlay has such a click on its way; a swipe or
 * a button inside the sheet does not, and those step aside at once.
 *
 * `pointer-events-none!`, with the bang: reka-ui puts `pointer-events: auto` on the
 * overlay as an inline style, which beats any class that is not `!important`.
 */
const { open } = injectDialogRootContext()
const passthrough = ref(false)
let pressedAt = -Infinity
const pressed = () => { pressedAt = performance.now() }
let timer: ReturnType<typeof setTimeout> | undefined
watch(open, (isOpen) => {
  clearTimeout(timer)
  passthrough.value = false
  if (isOpen) return
  if (performance.now() - pressedAt > 700) passthrough.value = true
  else timer = setTimeout(() => (passthrough.value = true), 80)
})
onBeforeUnmount(() => clearTimeout(timer))
</script>

<template>
  <DrawerOverlay
    data-slot="drawer-overlay"
    v-bind="delegatedProps"
    @pointerdown="pressed"
    :class="cn('data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 bg-black/10 fixed inset-0 z-50', passthrough && 'pointer-events-none!', props.class)"
  />
</template>
