/**
 * `GET /api/payment/session/{id}`. `shared::responses::payment::SessionStateResponse`.
 *
 * The verdict comes from Stripe rather than our own projection on purpose — the
 * projection lags the webhook, and from the outside "not confirmed yet" and "declined"
 * look identical.
 */
export type SessionStateResponse = {
  /** Stripe's own answer, not ours. */
  status: "complete" | "open" | "expired";
  /** Money actually arrived, as opposed to a complete session still processing. */
  paid: boolean;
  /** Present while the session is still payable. */
  clientSecret: string | null;
  /** So checkout can release the hold without the booking id being in the URL. */
  bookingId: string;
};
