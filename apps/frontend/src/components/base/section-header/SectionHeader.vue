<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import { cn } from "@/lib/utils"

/**
 * A label with an optional trailing action — "Nearby spots" / "See all".
 *
 * No cva: there is one shape and no variants, so a cva here would exist only to hold a
 * single string. Six sites today spell the alignment three ways (`justify-between
 * items-end`, `items-end justify-between`, `items-baseline justify-between`) and the
 * trailing action four ways, which is the entire problem this solves.
 *
 * Slots rather than `title`/`action` string props, so the label can be a Title at
 * whatever size the screen wants and the action can be a link, a button or a count.
 */
const props = withDefaults(defineProps<{
  as?: string | object
  class?: HTMLAttributes["class"]
}>(), {
  as: "div",
})
</script>

<template>
  <component
    :is="as"
    data-slot="section-header"
    :class="cn('flex items-end justify-between gap-2', props.class)"
  >
    <slot />
    <slot name="action" />
  </component>
</template>
