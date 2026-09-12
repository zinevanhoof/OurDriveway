import { loadStripe, type Stripe } from "@stripe/stripe-js";

/**
 * The Stripe.js instance, loaded once.
 *
 * `@stripe/stripe-js` is a loader, not the library: it injects a script tag pointing at
 * js.stripe.com. Stripe does not support bundling or self-hosting it — the whole point
 * is that the code handling card details is served by them and can be updated without a
 * redeploy here, which is also what keeps this app out of PCI scope.
 *
 * That has one consequence worth knowing: `src-tauri/tauri.conf.json` currently sets
 * `"csp": null`, so nothing blocks the load. If a CSP is ever added there, it needs
 * js.stripe.com in `script-src`, `frame-src` (the Element renders in an iframe) and
 * `connect-src`, or checkout silently renders nothing in the desktop and Android builds.
 *
 * The promise is created at module load and reused. Calling `loadStripe` per component
 * would re-resolve the same script for every checkout.
 */
const stripePromise = loadStripe(import.meta.env.VITE_STRIPE_PUBLISHABLE_KEY);

/**
 * Resolves the loaded Stripe instance.
 *
 * Rejects rather than returning null when it could not load — a blocked script or a
 * missing publishable key is not something checkout can degrade past, and a null here
 * would otherwise surface much later as an unreadable error inside `confirmPayment`.
 */
export async function stripe(): Promise<Stripe> {
  const loaded = await stripePromise;
  if (!loaded) {
    throw new Error(
      "Stripe.js failed to load. Check VITE_STRIPE_PUBLISHABLE_KEY and that js.stripe.com is reachable.",
    );
  }
  return loaded;
}
