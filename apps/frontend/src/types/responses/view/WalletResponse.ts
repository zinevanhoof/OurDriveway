import type { WalletTransactionResponse } from "./WalletTransactionResponse";

/**
 * `GET /api/view/account/wallet?month=YYYY-MM` — one month, which is also one page.
 *
 * `nextMonth` is the cursor: the next older month that holds anything, or null at the end
 * of the history. Asking for the month after it would be asking for nothing.
 */
export type WalletResponse = {
  /** `"YYYY-MM"`. */
  month: string;
  /** Everything that came in, positive. */
  inCents: number;
  /** Everything that went out, **positive**, and not counting withdrawals. */
  outCents: number;
  nextMonth: string | null;
  transactions: WalletTransactionResponse[];
};
