import { apiFetch } from "./king";
import { native } from "./http";
import { recordSeq } from "@/lib/awaitSeq";
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

// `earnings()` was here, behind `GET /api/payment/earnings`. It is
// `viewApi.fetchBalance()` now: payment-service writes and view-service reads, and the
// balance is a read. The same move gives it `pendingCents`, which this endpoint had no
// booking table to compute.

/**
 * Withdraws `amountCents` to the host's connected Stripe account.
 *
 * The amount is sent, unlike `createSession` above, because it is the host's own money
 * and they choose how much of it to take. It is not *trusted*: the server re-reads the
 * balance under an advisory lock and refuses anything larger rather than clamping, so
 * the figure returned is the only one that is true — print that one, never the one sent.
 *
 * Two of these racing is resolved by the same lock: the loser re-reads, finds nothing
 * left, and gets **422**. It is not a 409 and there is no retry to write — by the time
 * it answers, there genuinely is nothing to take out.
 *
 * `recordSeq` is what makes the wallet show the withdrawal on arrival. The response is
 * a 202: the payout row exists in payment-service, but the projection the wallet reads
 * is still catching up, and without recording the version here the very next balance
 * read can legitimately answer from before this write.
 *
 * The transfer itself has *not* happened yet when this resolves. The row lands as
 * pending and a worker turns it into paid or failed a moment later, which is why there
 * is nothing to await beyond the projection.
 */
export async function requestPayout(amountCents: number): Promise<number> {
  const res = await apiFetch("/api/payment/payout", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ amountCents }),
  });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));

  const body = await res.json();
  recordSeq(body.seq);
  return body.amountCents;
}

export type ConnectStatus = {
  /**
   * `needs_country` — no account, and no country on the profile to open one with.
   * Stripe fixes the country permanently when the account is created, so it is asked
   * for first rather than guessed.
   * `none` — ready to onboard; nothing exists at Stripe yet.
   * `onboarding` — an account exists but Stripe will not pay it: abandoned halfway, or
   * submitted and under review.
   * `enabled` — payouts are on, and the withdraw form is safe to show.
   */
  state: "needs_country" | "none" | "onboarding" | "enabled";
  /** Last four of the bank account Stripe pays into. Null when it hands back none. */
  bankLast4: string | null;
};

/**
 * Whether the caller can be paid, and where.
 *
 * Answered from Stripe live rather than from a projection — which is why it is on
 * payment-service and not view-service, like `sessionState` above and for the same
 * reason. A cached copy would go stale in exactly the moment that matters: a host
 * finishing onboarding in Stripe's own iframe and expecting the form to appear.
 */
export async function fetchConnectStatus(): Promise<ConnectStatus> {
  const res = await apiFetch("/api/payment/connect/account");
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  return await res.json();
}

/**
 * A client secret for Connect's embedded components, creating the caller's connected
 * account on the first call.
 *
 * POST, because it is not a read: the first one creates an account at Stripe. Called on
 * mount and again whenever a component asks for a fresh secret — see `lib/connect.ts`.
 *
 * Deliberately does **not** `recordSeq`: nothing here writes to a stream, and there is
 * no projection to wait for.
 */
export async function createAccountSession(): Promise<string> {
  const res = await apiFetch("/api/payment/connect/session", { method: "POST" });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  return (await res.json()).clientSecret;
}
