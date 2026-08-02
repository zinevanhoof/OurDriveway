<script lang="ts" setup>
import {
  CircleCheckIcon,
  InfoIcon,
  TriangleAlertIcon,
  OctagonXIcon,
  Loader2Icon,
  XIcon,
} from '@lucide/vue';


import { computed } from "vue"
import type { ToasterProps } from "vue-sonner"
import { Toaster as Sonner } from "vue-sonner"
import { cn } from "@/lib/utils"

const props = defineProps<ToasterProps>()

// Merged here rather than as a separate :toast-options binding: v-bind="props"
// already carries toastOptions, and specifying it twice makes one silently win.
// Caller values take precedence over the rounded-corner default.
const toasterProps = computed(() => ({
  ...props,
  toastOptions: {
    ...props.toastOptions,
    classes: { toast: "rounded-2xl", ...props.toastOptions?.classes },
  },
}))
</script>

<template>
  <Sonner
    v-bind="toasterProps"
    :class="cn('toaster group', props.class)"
    :style="{
      '--normal-bg': 'var(--popover)',
      '--normal-text': 'var(--popover-foreground)',
      '--normal-border': 'var(--border)',
      '--border-radius': 'var(--radius)',
      '--gray2': 'hsl(var(--popover) / 0.9)',
      '--gray3': 'var(--border)',
      '--gray4': 'var(--border)',
      '--gray5': 'var(--border)',
      '--gray12': 'var(--popover-foreground)',
    }"
  >
    <template #success-icon>
      <CircleCheckIcon class="size-4" />
    </template>
    <template #info-icon>
      <InfoIcon class="size-4" />
    </template>
    <template #warning-icon>
      <TriangleAlertIcon class="size-4" />
    </template>
    <template #error-icon>
      <OctagonXIcon class="size-4" />
    </template>
    <template #loading-icon>
      <div>
        <Loader2Icon class="size-4 animate-spin" />
      </div>
    </template>
    <template #close-icon>
      <XIcon class="size-4" />
    </template>
  </Sonner>
</template>
