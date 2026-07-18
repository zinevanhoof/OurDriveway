import { createApp, watch } from "vue";
import App from "./App.vue";
import { router } from "./router/index.ts";
import { createPinia } from "pinia";
import { MotionPlugin } from "motion-v";
import { autoAnimatePlugin } from "@formkit/auto-animate/vue";
import urql from "@urql/vue";
import { urqlClient } from "@/urql";
import { useAuthStore } from "@/stores/auth";

import "@/main.css";
import { installNativeFetch } from "./api/http.ts";
import { refreshAccessToken } from "./api/refresh.ts";
import { fetchMe } from "./api/me.ts";

const safeTop = parseFloat(
  getComputedStyle(document.documentElement).getPropertyValue("--safe-top") ||
    "0",
);

const safeBottom = parseFloat(
  getComputedStyle(document.documentElement).getPropertyValue(
    "--safe-bottom",
  ) || "0",
);

async function bootstrap() {
  // Route all fetches through plugin-http on native, before the first request
  // (refreshAccessToken below) fires. No-op on web.
  installNativeFetch();

  const app = createApp(App);

  const pinia = createPinia();

  app.use(pinia);
  app.use(urql, urqlClient);
  app.use(MotionPlugin);
  app
    .use(autoAnimatePlugin)
    .provide("safeTop", safeTop)
    .provide("safeBottom", safeBottom);

  const auth = useAuthStore();

  if (await refreshAccessToken()) auth.setUser(await fetchMe());

  app.use(router);

  // The router guard only covers navigations, so it misses a session that dies
  // on the page you're already on (a 401 retry that fails to refresh).
  watch(
    () => auth.isAuthenticated,
    (ok) => {
      if (!ok) router.push({ name: "login" });
    },
  );

  await router.isReady();

  app.mount("#app");
}

bootstrap();
