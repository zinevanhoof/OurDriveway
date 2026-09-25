<script setup lang="ts">
import { ref, useTemplateRef, watch } from "vue"
import { useIntersectionObserver } from "@vueuse/core"

/**
 * Infinite scroll: put this after the rows of a `useInfiniteQuery` list and it asks for
 * the next page whenever it is on screen.
 *
 * ```vue
 * <Sentinel :has-next-page="hasNextPage" :fetching="isFetchingNextPage" @load="fetchNextPage" />
 * ```
 *
 * The observer only records *whether* it is visible; a watcher decides whether to ask.
 * Split that way because an IntersectionObserver reports transitions, and a first page
 * that does not fill the screen leaves the sentinel visible with no further callback
 * ever coming. As watched state, the condition is re-checked when `hasNextPage` flips,
 * which keeps loading until the sentinel is pushed off screen.
 *
 * It takes up no space under any spacing: see the `data-slot="sentinel"` rules in
 * `main.css`.
 */
const props = defineProps<{ hasNextPage: boolean; fetching: boolean }>()
const emit = defineEmits<{ load: [] }>()

const visible = ref(false)
useIntersectionObserver(useTemplateRef<HTMLElement>("el"), ([entry]) => {
  visible.value = !!entry?.isIntersecting
})

watch(
  [visible, () => props.hasNextPage, () => props.fetching],
  () => {
    if (visible.value && props.hasNextPage && !props.fetching) emit("load")
  },
  { immediate: true },
)
</script>

<template>
  <div ref="el" data-slot="sentinel" aria-hidden="true" />
</template>
