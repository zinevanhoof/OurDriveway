import type { Booked } from "@/types/domain/spot";

/**
 * One line of the wallet: a single movement of money involving the caller.
 *
 * `GET /api/view/me/payouts` and its `PayoutListItem` are gone — withdrawals are one of
 * the four kinds below, in the same list as everything else that moved.
 */
export type WalletTransactionResponse = {
  /** `<uuid>:<kind>`. One payment yields two rows — the charge and its refund. */
  id: string;
  /**
   * What it is, which decides the icon. The *direction* is the sign of `amountCents`,
   * because a refund is money back to a renter and money away from a host.
   *
   * Narrower than the wire, which is a bare `String` carrying one of
   * `shared/src/projections/wallet.rs`'s four constants. Narrowing it here is the
   * client's call and it is the right one — but it is a cast, not a check: a fifth kind
   * added server-side would arrive as a value this union says is impossible.
   */
  kind: "in" | "out" | "refund" | "payout";
  /** Signed EUR cents, from the caller's point of view: what their balance did. */
  amountCents: number;
  /** ISO 8601. What the month grouping and the ordering are on. */
  occurredAt: string;
  /** Host income that has not settled yet, so it is not withdrawable. */
  pending: boolean;
  /** The spot's title. Null on a payout, and on a spot not projected yet. */
  title: string | null;
  /** The booking's slots, in the spot's own wall clock. Null on a payout. */
  booked: Booked | null;
  /** The spot's IANA zone, which is what `booked` is written in. */
  timezone: string | null;
};
