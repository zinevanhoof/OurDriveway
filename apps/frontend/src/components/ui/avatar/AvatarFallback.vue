<script setup lang="ts">
import type { AvatarFallbackProps } from 'reka-ui'
import { computed, type HTMLAttributes } from 'vue'
import { reactiveOmit } from '@vueuse/core'
import { AvatarFallback } from 'reka-ui'
import { cn } from '@/lib/utils'
import { initialsOf } from '@/lib/initials'

const props = defineProps<AvatarFallbackProps & {
  class?: HTMLAttributes['class']
  name?: { firstName: string; lastName: string }
}>()

const delegatedProps = reactiveOmit(props, 'class', 'name')

// Empty when no name (slot used), and empty rather than throwing when the person
// has not loaded yet — see `initialsOf`, which is where that case is tested.
const initials = computed(() => initialsOf(props.name))
</script>

<template>
  <AvatarFallback data-slot="avatar-fallback" v-bind="delegatedProps"
    :class="cn('bg-primary text-primary-foreground font-bold rounded-full flex size-full items-center justify-center text-sm group-data-[size=sm]/avatar:text-xs group-data-[size=xl]/avatar:text-base group-data-[size=2xl]/avatar:text-lg group-data-[size=3xl]/avatar:text-xl', props.class)">
    <slot>{{ initials }}</slot>
  </AvatarFallback>
</template>
