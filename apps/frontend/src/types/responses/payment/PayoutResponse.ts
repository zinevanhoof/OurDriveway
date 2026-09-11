/**
 * `POST /api/payment/payout`. `shared::responses::payment::PayoutResponse`.
 *
 * The amount is here and not merely echoed back: the client asks for a figure but the
 * server recomputes under the lock and may pay **less** — a booking that had not settled
 * when the page was drawn is gone from the balance by the time the request arrives. This
 * is the only figure that is true, so print this one, never the one sent.
 */
export type PayoutResponse = {
  amountCents: number;
};
