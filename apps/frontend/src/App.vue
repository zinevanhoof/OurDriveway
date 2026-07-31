<script setup lang="ts">
import { computed } from "vue";
import { Toaster } from "vue-sonner";
import { native } from "./api/http";
import MobileLayout from "./layouts/MobileLayout.vue";
import WebLayout from "./layouts/WebLayout.vue";

import 'vue-sonner/style.css'

// Toasts drop from the top; passing `top` replaces vue-sonner's default gap, so on
// native (mobile) we add the safe-area inset (--safe-top) to clear the native status bar.
const toastOffset = computed(() =>
  true ? { top: "calc(var(--safe-top) + 1rem)" } : undefined,
);
</script>

<template>
  <MobileLayout v-if="true" />
  <WebLayout v-else />
  <Toaster position="top-center" :offset="toastOffset" :mobile-offset="toastOffset" />
</template>

<style>
#app {
  height: 100dvh;
  display: flex;
  flex-direction: column;
  overflow: hidden;

  padding-top: var(--safe-top);
  padding-bottom: var(--safe-bottom);
}
</style>