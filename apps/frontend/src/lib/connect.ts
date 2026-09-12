import {
  loadConnectAndInitialize,
  type ConnectElementTagName,
  type ConnectHTMLElementRecord,
} from "@stripe/connect-js";

import { createAccountSession } from "@/api/paymentApi";
import { cssColorToHex, cssLengthToPx, token } from "@/lib/theme";

/**
 * Stripe Connect's embedded components — the host's side of the money.
 *
 * The sibling of `lib/stripe.ts`, and everything said there applies here too: this is a
 * loader, not the library. It injects a script from connect-js.stripe.com, and the code
 * that renders identity and bank-account forms is served by Stripe rather than bundled,
 * which is what keeps this app out of that compliance scope as well.
 *
 * **CSP:** `src-tauri/tauri.conf.json` currently sets `"csp": null`, so nothing blocks
 * the load. If one is ever added it needs `connect-js.stripe.com` in `script-src`,
 * `frame-src` (every component renders in an iframe) and `connect-src` — alongside the
 * `js.stripe.com` entries `lib/stripe.ts` already documents.
 *
 * Why components and not a redirect: the alternative is an Account Link, which sends
 * the host to connect.stripe.com and back. Both work; this one keeps onboarding inside
 * the app, which is the whole reason the withdraw screen can be self-contained.
 */

/**
 * Mounts one embedded component into `container`, on a Connect instance of its own.
 *
 * # Why an instance per mount, and not one memoised for the module
 *
 * `loadConnectAndInitialize` calls `fetchClientSecret` **once, eagerly**, at
 * initialisation, and that single Account Session is what the component's iframe
 * *claims* when it renders. A session can be claimed once.
 *
 * A module-scoped instance therefore outlives the screen that mounted it, and the next
 * mount — navigating back to the withdraw page, or a Vite HMR update, both of which
 * rebuild the component while module state survives — hands a second iframe a secret
 * that has already been used. Stripe answers exactly that:
 *
 *     Failed to claim account session. This is expected if the account session is
 *     invalid, expired or has been used…
 *
 * So the instance is created here, per call, and dies with the element it made. The
 * cost is one `POST /api/payment/connect/session` per mount, which is also what makes
 * the secret fresh — the endpoint is get-or-create, so no extra Stripe *account* is
 * ever made by asking again.
 *
 * Returns the element, so a caller can attach listeners to it (`setOnExit`).
 */
export function mountConnect<T extends ConnectElementTagName>(
  tagName: T,
  container: HTMLElement,
): ConnectHTMLElementRecord[T] {
  // `--radius` in pixels, which is the only unit Connect's appearance API takes. Read
  // once: three variables want the same number.
  const radius = cssLengthToPx(token("--radius"), 24);

  const instance = loadConnectAndInitialize({
    publishableKey: import.meta.env.VITE_STRIPE_PUBLISHABLE_KEY,
    // A callback rather than a value, and Stripe calls it again when the session
    // expires — so a host who leaves onboarding open resumes instead of hitting a dead
    // iframe. The server creates their connected account on the first of these calls;
    // see `ConnectService::account_session`.
    fetchClientSecret: createAccountSession,
    // Themed off the app's own tokens, for the same reason the Payment Element is —
    // and with the same conversion, because Stripe rejects Tailwind's `oklch(...)`.
    appearance: {
      variables: {
        colorPrimary: cssColorToHex(token("--primary")),
        colorBackground: cssColorToHex(token("--card")),
        colorText: cssColorToHex(token("--foreground")),
        colorDanger: cssColorToHex(token("--destructive")),
        // Converted, not passed through: Connect takes pixels only and our token is
        // `0.75rem`. 24px is its documented ceiling for these.
        //
        // All three, because `borderRadius` is only the *general* one — form fields and
        // buttons have their own variables and do not inherit it. Left unset, Connect's
        // own defaults win, which renders inputs almost as pills next to everything else
        // on the screen.
        borderRadius: radius,
        formBorderRadius: radius,
        buttonBorderRadius: radius,
      },
    },
  });

  const element = instance.create(tagName);

  // `stripe-connect-*` is an unknown element to the browser, so it is `display: inline`
  // — and an inline box does not constrain the iframe Stripe puts inside it, which then
  // renders wider than the container and spills past the page's padding. Block-level and
  // full width is the whole fix, and it belongs here rather than in a stylesheet: this
  // is the one place that knows these tag names.
  element.style.display = "block";
  element.style.width = "100%";
  // `replaceChildren` rather than `append`: a container that somehow already holds a
  // component is being remounted, and two iframes racing for one session is the very
  // failure this function exists to avoid.
  container.replaceChildren(element);
  return element;
}
