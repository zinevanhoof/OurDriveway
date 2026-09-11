/**
 * `POST /api/payment/payout`. `shared::requests::payment::PayoutRequest`.
 *
 * The amount *is* sent, unlike `CreateSessionRequest` — it is the host's own money and
 * they choose how much of it to take. It is not trusted: the server re-reads the balance
 * under an advisory lock and refuses anything larger rather than clamping, so the figure
 * in `PayoutResponse` is the only one that is true.
 */
export type PayoutRequest = {
  amountCents: number;
};
