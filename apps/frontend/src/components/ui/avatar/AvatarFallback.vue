<script setup lang="ts">
import type { AvatarFallbackProps } from 'reka-ui'
import { computed, type HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { AvatarFallback } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<AvatarFallbackProps & {
  class?: HTMLAttributes['class']
  name?: { firstName: string; lastName: string }
}>()

const delegatedProps = reactiveOmit(props, 'class', 'name')

// One letter per word: { firstName: "Zine", lastName: "Van Hoof" } -> "ZVH".
// Empty when no name (slot used).
const initials = computed(() =>
  props.name
    ? `${props.name.firstName} ${props.name.lastName}`.trim().split(/\s+/).map(w => w[0].toUpperCase()).join('')
    : ''
)
</script>

<template>
  <AvatarFallback data-slot="avatar-fallback" v-bind="delegatedProps"
    :class="cn('bg-primary text-primary-foreground font-bold rounded-full flex size-full items-center justify-center text-sm group-data-[size=sm]/avatar:text-xs', props.class)">
    <slot>{{ initials }}</slot>
  </AvatarFallback>
</template>
