<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import type { TitleVariants } from "."
import { cn } from "@/lib/utils"
import { titleVariants } from "."

/**
 * `as` defaults to `div` rather than a heading on purpose — a title is not always a
 * heading, and silently emitting `<h2>` for a row label would be worse than emitting a
 * div. But it makes the fix available: there is currently exactly one `<h1>` in the
 * whole non-`ui` tree, so most screens announce no structure at all.
 */
const props = withDefaults(defineProps<{
  as?: string | object
  class?: HTMLAttributes["class"]
  size?: TitleVariants["size"]
  weight?: TitleVariants["weight"]
  tone?: TitleVariants["tone"]
}>(), {
  as: "div",
  size: "md",
  weight: "bold",
  tone: "default",
})
</script>

<template>
  <component
    :is="as"
    data-slot="title"
    :class="cn(titleVariants({ size, weight, tone }), props.class)"
  >
    <slot />
  </component>
</template>
