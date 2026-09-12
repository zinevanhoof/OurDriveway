<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import type { SurfaceVariants } from "."
import { cn } from "@/lib/utils"
import { surfaceVariants } from "."

/**
 * `as` rather than reka's `Primitive`: `<component :is>` is built into Vue and does
 * everything needed here. Primitive earns its keep in `ui/` because reka needs it for
 * `asChild` slot merging; nothing in `base/` does.
 *
 * `interactive` deliberately does NOT imply `as="button"`. A tappable row is sometimes a
 * `<button>` and sometimes a `<RouterLink>`, and picking for the caller would mean the
 * link case has to fight the component.
 */
const props = withDefaults(defineProps<{
  as?: string | object
  class?: HTMLAttributes["class"]
  variant?: SurfaceVariants["variant"]
  size?: SurfaceVariants["size"]
  orientation?: SurfaceVariants["orientation"]
  interactive?: boolean
}>(), {
  as: "div",
  variant: "card",
  size: "md",
  orientation: "vertical",
  interactive: false,
})
</script>

<template>
  <component
    :is="as"
    data-slot="surface"
    :data-variant="variant"
    :data-size="size"
    :data-orientation="orientation"
    :class="cn(surfaceVariants({ variant, size, orientation, interactive }), props.class)"
  >
    <slot />
  </component>
</template>
