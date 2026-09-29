<script setup lang="ts">
import { Toaster } from "vue-sonner";
import MobileLayout from "./layouts/MobileLayout.vue";

import 'vue-sonner/style.css'

// The phone shell, everywhere the app itself runs: native, a phone's browser, and the
// frame WebLayout puts it in on a bigger screen. `main.ts` decides which of App and
// WebLayout to mount, so this never has to.
</script>

<template>
  <MobileLayout />
  <Toaster position="top-center" />
</template>

<style>
#app {
  height: 100dvh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
</style>