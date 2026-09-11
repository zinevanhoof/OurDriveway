/**
 * `GET /api/payment/connect/account`.
 * `shared::responses::payment::ConnectStatusResponse`.
 *
 * Answered from Stripe live rather than from a projection, which is why it is on
 * payment-service and not view-service. A cached copy would go stale in exactly the
 * moment that matters: a host finishing onboarding in Stripe's own iframe and expecting
 * the form to appear.
 */
export type ConnectStatusResponse = {
  /**
   * `needs_country` — no account, and no country on the profile to open one with. Stripe
   * fixes the country permanently when the account is created, so it is asked for first
   * rather than guessed.
   * `none` — ready to onboard; nothing exists at Stripe yet.
   * `onboarding` — an account exists but Stripe will not pay it: abandoned halfway, or
   * submitted and under review.
   * `enabled` — payouts are on, and the withdraw form is safe to show.
   */
  state: "needs_country" | "none" | "onboarding" | "enabled";
  /** Last four of the bank account Stripe pays into. Null when it hands back none. */
  bankLast4: string | null;
};
