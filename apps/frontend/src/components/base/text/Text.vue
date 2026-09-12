<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import type { TextVariants } from "."
import { cn } from "@/lib/utils"
import { textVariants } from "."

/**
 * Defaults to a `<div>`, not a `<p>`: a good third of these sit inside a flex row next
 * to an icon, and a paragraph there is a lie about the content. Pass `as="p"` for real
 * prose.
 *
 * No line-clamp in the base. Two of the app's descriptions want to clamp and the rest
 * want to wrap freely, and a clamp is one `line-clamp-2` at the call site — whereas
 * undoing an unwanted one means knowing that it sets `display` and fighting it.
 */
const props = withDefaults(defineProps<{
  as?: string | object
  class?: HTMLAttributes["class"]
  size?: TextVariants["size"]
  weight?: TextVariants["weight"]
  tone?: TextVariants["tone"]
}>(), {
  as: "div",
  size: "xs",
  weight: "medium",
  tone: "muted",
})
</script>

<template>
  <component
    :is="as"
    data-slot="text"
    :class="cn(textVariants({ size, weight, tone }), props.class)"
  >
    <slot />
  </component>
</template>
