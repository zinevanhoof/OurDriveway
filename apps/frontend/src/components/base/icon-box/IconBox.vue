<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import type { IconBoxVariants } from "."
import { cn } from "@/lib/utils"
import { iconBoxVariants } from "."

const props = withDefaults(defineProps<{
  class?: HTMLAttributes["class"]
  size?: IconBoxVariants["size"]
  tone?: IconBoxVariants["tone"]
  shape?: IconBoxVariants["shape"]
}>(), {
  size: "md",
  tone: "accent",
  shape: "square",
})
</script>

<template>
  <div
    data-slot="icon-box"
    :class="cn(iconBoxVariants({ size, tone, shape }), props.class)"
  >
    <slot />
  </div>
</template>
