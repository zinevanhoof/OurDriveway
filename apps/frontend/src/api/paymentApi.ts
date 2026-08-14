import { apiFetch } from "./king";
import { native } from "./http";
import { readErrorDetail } from "@/lib/serverErrors";

/**
 * Where Stripe sends the renter back after a redirect payment method.
 *
 * Built here rather than by callers so there is one shape, not one per call site — it was
 * briefly duplicated and the two forms had already diverged.
 *
 * `{CHECKOUT_SESSION_ID}` is a literal: Stripe substitutes the real id when it redirects,
 * which is how `/checkout` gets the only handle it needs.
 *
 * Native returns to `checkout/return.html`, a static page that opens the app's
 * `ourdriveway://` scheme, rather than naming that scheme here. Two layers of Stripe
 * disagree about custom schemes: `POST /v1/checkout/sessions` stores one without
 * complaint, but Stripe.js rejects it at confirm time with "invalid returnUrl". Only the
 * http(s) form survives both.
 *
 * That page is reached in the *system browser*, never in the webview — `CheckoutComponent`
 * cancels the off-origin navigation and hands the bank URL to the OS, so the bank, Stripe
 * and the wrapper all load in Chrome, which unlike a webview does dispatch a custom scheme
 * back to the OS.
 *
 * Which makes `VITE_APP_ORIGIN` load-bearing on native and unlike the web case it has no
 * usable fallback: `window.location.origin` inside Tauri is the webview's own origin, which
 * no phone browser can resolve. It must be reachable from the *device* — a LAN address in
 * development, the public domain in production.
 */
function returnUrl(): string {
  // `VITE_APP_ORIGIN` when set, so a dev build can pin the origin Stripe returns to —
  // the vite server on :1420 — rather than whatever host the page happens to be open on.
  // Falling back to the live origin keeps LAN testing through Caddy working without a
  // rebuild, which a hardcoded origin would break.
  const origin = import.meta.env.VITE_APP_ORIGIN || window.location.origin;
  const path = native ? "/checkout/return.html" : "/checkout";

  return `${origin}${path}?session_id={CHECKOUT_SESSION_ID}`;
}

export type NewSession = {
  /** The only handle checkout carries in its URL. */
  sessionId: string;
  clientSecret: string;
};

/**
 * Starts payment for a booking that is already held, by creating a Checkout Session.
 *
 * The amount is never sent: the server takes it from the booking as it priced it at
 * reserve time, so the figure on screen is display and the figure charged is the
 * server's. Idempotent per booking — calling this again returns the *same* session,
 * which is what makes "Continue payment" on a reserved booking safe.
 *
 * The return URL is decided here rather than server-side, because only this side knows
 * whether it is running in a browser or in Tauri — see `returnUrl` above.
 *
 * Deliberately does **not** `recordSeq`. This client reads nothing from the PAYMENTS
 * stream, so there is no write to wait for, and echoing a PAYMENTS position on
 * subsequent reads would make view-service block on a projection nothing needs.
 */
export async function createSession(bookingId: string): Promise<NewSession> {
  const res = await apiFetch("/api/payment/session", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ bookingId, returnUrl: returnUrl() }),
  });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));

  return await res.json();
}

export type SessionState = {
  /** Stripe's own answer, not ours. */
  status: "complete" | "open" | "expired";
  /** Money actually arrived, as opposed to a complete session still processing. */
  paid: boolean;
  /** Present while the session is still payable. */
  clientSecret: string | null;
  /** So checkout can release the hold without the booking id being in the URL. */
  bookingId: string;
};

/**
 * What became of a checkout.
 *
 * The whole reason checkout needs nothing but a session id in its URL: this turns that
 * id back into the client secret to mount against, the booking to release, and Stripe's
 * verdict on whether it was paid.
 *
 * The verdict comes from Stripe rather than our own projection on purpose — the
 * projection lags the webhook, and from the outside "not confirmed yet" and "declined"
 * look identical. 404 for a session that isn't yours.
 */
export async function sessionState(sessionId: string): Promise<SessionState> {
  const res = await apiFetch(`/api/payment/session/${sessionId}`);
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  return await res.json();
}

export type Earnings = {
  /** Withdrawable now: settled income minus what has already been taken out. */
  availableCents: number;
  /** Everything earned and settled, ever. */
  earnedCents: number;
  paidOutCents: number;
};

/**
 * What this host has earned and what they can withdraw.
 *
 * The single source for every money figure shown to a host. It deliberately does not
 * come from the booking read model: a total derived one way next to a withdraw button
 * that spends a total derived another way is how the two end up disagreeing.
 */
export async function earnings(): Promise<Earnings> {
  const res = await apiFetch("/api/payment/earnings");
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  return await res.json();
}

/**
 * Withdraws the whole available balance. Nothing real moves — no bank, no transfer.
 *
 * No amount is sent: the server computes it, so there is nothing here a caller could
 * inflate. Two of these racing is resolved server-side by a compare-and-swap, which
 * surfaces as a 409 rather than a double payout.
 */
export async function requestPayout(): Promise<number> {
  const res = await apiFetch("/api/payment/payout", { method: "POST" });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  return (await res.json()).amountCents;
}
