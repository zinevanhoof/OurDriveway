<script setup lang="ts">
/**
 * One deviation from stock shadcn: the background is `bg-card`, not `bg-transparent`.
 *
 * Sixteen of the app's inputs were passing `class="bg-card"` to undo that default, and
 * the ones that were not sit on a surface already painted `--card` — so transparent was
 * the exception nothing actually wanted. `--popover` holds the same value as `--card` in
 * both themes, which is why the drawer- and map-hosted inputs look identical either way.
 *
 * `dark:bg-input/30` still wins in dark mode: a `dark:` variant does not collide with an
 * unprefixed `bg-*`, so tailwind-merge keeps both and the variant applies.
 */
import type { HTMLAttributes } from "vue"
import { useVModel } from "@vueuse/core"
import { cn } from "@/lib/utils"

const props = defineProps<{
  defaultValue?: string | number
  modelValue?: string | number
  class?: HTMLAttributes["class"]
}>()

const emits = defineEmits<{
  (e: "update:modelValue", payload: string | number): void
}>()

const modelValue = useVModel(props, "modelValue", emits, {
  passive: true,
  defaultValue: props.defaultValue,
})
</script>

<template>
  <input v-model="modelValue" data-slot="input" :class="cn(
    'dark:bg-input/30 border-input focus-visible:border-ring focus-visible:ring-ring/15 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 h-9 rounded-md border bg-card px-2.5 py-1 text-base shadow-xs transition-[color,box-shadow] file:h-7 file:text-sm file:font-medium focus-visible:ring-3 aria-invalid:ring-3 md:text-sm w-full min-w-0 outline-none file:inline-flex file:border-0 file:bg-transparent file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50',
    props.class,
  )">
</template>
