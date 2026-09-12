/**
 * `GET /api/view/host/balance`.
 *
 * Two figures, not four: `earnedCents` and `paidOutCents` are the arithmetic behind
 * `availableCents` and no screen renders them, so they stop at the server.
 */
export type BalanceResponse = {
  /** Withdrawable now: settled income minus what has already been taken out. */
  availableCents: number;
  /** Earned but not settled yet — what `availableCents` will grow by. */
  pendingCents: number;
};
