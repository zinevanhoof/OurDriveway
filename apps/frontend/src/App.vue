<script setup lang="ts">
import { Toaster } from "vue-sonner";
import MobileLayout from "./layouts/MobileLayout.vue";
import WebLayout from "./layouts/WebLayout.vue";

import 'vue-sonner/style.css'

/**
 * Which shell the app renders in. Hardcoded, and deliberately **not** `native`.
 *
 * The two questions are different and were conflated while this was a bare `true`:
 *
 *   - *Is this Tauri?* — a capability question. `native` from `@/api/http` answers it,
 *     and it gates things that genuinely differ: the plugin-http fetch override, the
 *     platform geolocation prompt, opening a maps app, and the `ourdriveway://` deep
 *     link a redirect payment returns through.
 *   - *Which layout?* — a design question, and the answer is currently "the mobile one,
 *     everywhere". The phone shell is what the app is designed around, and it reads fine
 *     in a browser, because the window is the whole viewport on either.
 *
 * Tying the layout to `native` would have meant the web build silently rendering
 * `WebLayout`, which is a placeholder. Flip this to `native` the day WebLayout is real.
 */
const MOBILE_SHELL = true;
</script>

<template>
  <MobileLayout v-if="MOBILE_SHELL" />
  <WebLayout v-else />
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