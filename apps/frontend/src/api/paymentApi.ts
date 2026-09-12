import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";

import { get, post } from "./client";
import { native } from "./http";
import { viewKeys } from "./keys";
import type { AccountSessionResponse } from "@/types/responses/payment/AccountSessionResponse";
import type { ConnectStatusResponse } from "@/types/responses/payment/ConnectStatusResponse";
import type { CreateSessionResponse } from "@/types/responses/payment/CreateSessionResponse";
import type { PayoutResponse } from "@/types/responses/payment/PayoutResponse";
import type { SessionStateResponse } from "@/types/responses/payment/SessionStateResponse";

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
 * The origin is always the page's own, on both platforms. On native the whole redirect runs
 * *inside* the webview — the bank, Stripe and that return page all load there — and
 * `catch_deep_link` in `src-tauri/src/lib.rs` turns the `ourdriveway://` hop into an in-app
 * navigation. The webview can always reach its own origin: the dev machine's LAN address
 * under `tauri android dev`, `http://tauri.localhost` in a release build.
 *
 * It used to be pinned by `VITE_APP_ORIGIN`, set to `http://localhost:1420` for dev. On a
 * phone that is the phone itself, so the return page never loaded and checkout hung after
 * the bank step.
 */
function returnUrl(): string {
  const path = native ? "/checkout/return.html" : "/checkout";

  return `${window.location.origin}${path}?session_id={CHECKOUT_SESSION_ID}`;
}

/**
 * Starts payment for a booking that is already held, by creating a Checkout Session.
 *
 * Idempotent per booking — calling this again returns the *same* session, which is what
 * makes "Continue payment" on a reserved booking safe.
 *
 * The server sends no `X-Version` on this, which is the enforcement of what used to be a
 * comment: this client reads nothing from the PAYMENTS stream, so there is no write to
 * wait for, and echoing a PAYMENTS position on subsequent reads would make view-service
 * block on a projection nothing needs.
 */
export const createSession = (bookingId: string) =>
  post<CreateSessionResponse>("/api/payment/session", {
    bookingId,
    returnUrl: returnUrl(),
  });

/**
 * What became of a checkout.
 *
 * The whole reason checkout needs nothing but a session id in its URL: this turns that
 * id back into the client secret to mount against, the booking to release, and Stripe's
 * verdict on whether it was paid. 404 for a session that isn't yours.
 */
export const sessionState = (sessionId: string) =>
  get<SessionStateResponse>(`/api/payment/session/${sessionId}`);

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
 * left, and gets **409**. There is no retry to write — by the time it answers, there
 * genuinely is nothing to take out. (It was a 422 until 422 became garde's alone.)
 *
 * The transfer itself has *not* happened when this resolves. The row lands as pending and
 * a worker turns it into paid or failed a moment later, which is why there is nothing to
 * await beyond the projection.
 */
export const requestPayout = (amountCents: number) =>
  post<PayoutResponse>("/api/payment/payout", { amountCents });

/**
 * Whether the caller can be paid, and where.
 *
 * Answered from Stripe live rather than from a projection — which is why it is on
 * payment-service and not view-service, like `sessionState` above and for the same
 * reason. A cached copy would go stale in exactly the moment that matters: a host
 * finishing onboarding in Stripe's own iframe and expecting the form to appear.
 */
export const fetchConnectStatus = () =>
  get<ConnectStatusResponse>("/api/payment/connect/account");

/**
 * A client secret for Connect's embedded components, creating the caller's connected
 * account on the first call.
 *
 * POST, because it is not a read: the first one creates an account at Stripe. Called on
 * mount and again whenever a component asks for a fresh secret — see `lib/connect.ts`,
 * which is also why this stays a plain function: it is passed to Stripe as a callback,
 * outside any component's setup, where a hook cannot go.
 */
export const createAccountSession = async () =>
  (await post<AccountSessionResponse>("/api/payment/connect/session"))
    .clientSecret;

// ─── hooks ──────────────────────────────────────────────────────────────────

export const useCreateSession = () => useMutation({ mutationFn: createSession });

/**
 * `staleTime: 0` on purpose. Everything else in the app tolerates a stale window
 * because `X-Await-Version` holds a read until this client's own writes land — but
 * this answer comes from Stripe, which we never wrote to, so there is no version to
 * wait on and no reason to trust a cached copy.
 */
export const useConnectStatus = () =>
  useQuery({
    queryKey: viewKeys.connectAccount,
    queryFn: fetchConnectStatus,
    staleTime: 0,
  });

export function useRequestPayout() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: requestPayout,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: viewKeys.balance });
      queryClient.invalidateQueries({ queryKey: viewKeys.wallet });
    },
  });
}
