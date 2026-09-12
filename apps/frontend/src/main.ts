import { createApp, watch } from "vue";
import App from "./App.vue";
import { router } from "./router/index.ts";
import { createPinia } from "pinia";
import { MotionPlugin } from "motion-v";
import { autoAnimatePlugin } from "@formkit/auto-animate/vue";
import { VueQueryPlugin, QueryClient } from "@tanstack/vue-query";
import { useAuthStore } from "@/stores/auth";

import "@/main.css";
import { installNativeFetch, native } from "./api/http.ts";
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

  // Replaced the urql client and its exchange chain. Two of those three exchanges have
  // no equivalent here because they have no job left: `authExchange` refreshed a token
  // on a 401, which `api/client.ts` already does for every request in the app; and
  // `cacheExchange` (graphcache) normalized entities by id, which only mattered because
  // one GraphQL document could return a `user` that another had already fetched.
  //
  // **That normalization is the real behavioural change.** graphcache kept one `user`
  // entity backing every reference to that person, so updating a profile updated it
  // everywhere at once. vue-query caches per query key instead, so a write invalidates
  // keys — see `viewKeys` in api/keys.ts.
  app.use(VueQueryPlugin, {
    queryClient: new QueryClient({
      defaultOptions: {
        queries: {
          // Reads are served from a projection that lags a write by however long the
          // outbox relay and the projector take. `X-Await-Version` already holds a
          // request until this client's own writes are visible, so a short stale
          // window costs nothing and saves a refetch on every remount.
          staleTime: 30_000,
          // A 401 the transport could not refresh has already logged the user out,
          // and the router sends them to login. Retrying that is noise.
          retry: 1,
          refetchOnWindowFocus: false,
        },
      },
    }),
  });

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

  await installDeepLinks();

  app.mount("#app");
}

/**
 * Brings the app back after a redirect payment.
 *
 * Bancontact and iDEAL leave for the customer's bank, and Stripe's `return_url` would
 * otherwise land in the system browser — where this app has no session at all, because
 * `installNativeFetch` keeps the refresh cookie in reqwest's jar inside the Tauri process.
 * A `ourdriveway://checkout?session_id=…` link routes the return here instead.
 *
 * The incoming URL carries the same path and query the web build would have navigated to,
 * so this only has to hand them to the router — checkout needs nothing else, which is the
 * whole point of the session id being its only handle.
 *
 * Only fires when the payment actually left the app. While the redirect stays inside the
 * webview, the scheme never reaches the OS (ERR_UNKNOWN_URL_SCHEME) and nothing arrives
 * here — see the note on `returnUrl` in api/paymentApi.ts.
 *
 * Native only. `isTauri()` is false on the web, where the redirect simply comes back to
 * the origin it left.
 */
async function installDeepLinks() {
  if (!native) return;

  const { onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
  await onOpenUrl((urls) => {
    for (const raw of urls) {
      try {
        const url = new URL(raw);
        // `ourdriveway://checkout?session_id=…` parses with "checkout" as the *host*, not
        // the path — a custom scheme has no authority — so both have to be joined back
        // together before the router sees it.
        //
        // The collapse is not decoration: written with three slashes
        // (`ourdriveway:///checkout`) the host is empty and the pathname carries the whole
        // path, which would otherwise produce `//checkout` and fail to match any route.
        // Accepting both spellings means a malformed return URL degrades to working.
        const path =
          `/${url.host}${url.pathname}`.replace(/\/{2,}/g, "/").replace(/(.)\/$/, "$1");
        router.push({ path, query: Object.fromEntries(url.searchParams) });
      } catch {
        // A malformed link is not worth breaking startup over.
      }
    }
  });
}

bootstrap();
