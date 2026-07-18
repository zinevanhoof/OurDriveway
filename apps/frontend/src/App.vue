<script setup lang="ts">
import { computed } from "vue";
import { Toaster } from "vue-sonner";
import { useIsMobile } from "./composables/useIsMobile";
import { useAuthStore } from "./stores/auth";
import MobileLayout from "./layouts/MobileLayout.vue";
import WebLayout from "./layouts/WebLayout.vue";

import 'vue-sonner/style.css'

const { isMobile } = useIsMobile();
const auth = useAuthStore();

// The Toaster is viewport-fixed, so lift bottom toasts above the mobile navbar
// (h-10 = 2.5rem, shown only when mobile + authenticated) plus the safe-area inset.
const toastOffset = computed(() =>
  isMobile.value && auth.isAuthenticated
    ? { bottom: "calc(var(--safe-bottom))" }
    : undefined,
);
</script>

<template>
  <MobileLayout v-if="isMobile" />
  <WebLayout v-else />
  <Toaster :offset="toastOffset" :mobile-offset="toastOffset" />
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