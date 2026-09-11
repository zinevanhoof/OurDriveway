/**
 * Query keys, in one place.
 *
 * vue-query caches by key, so a key is what a write has to name to invalidate a
 * read. Kept together because the two ends are otherwise a string literal in a
 * component and a matching one in a mutation handler, which is exactly the pair
 * that drifts — and did: the connect-status query had a hand-written
 * `['connect','account']` in `WalletWithdrawComponent` while everything else
 * came from here.
 *
 * Here rather than in `viewApi.ts`, where this lived, because the mutations that
 * invalidate these keys are in `spotApi`, `bookingApi`, `userApi` and
 * `paymentApi` — importing `viewApi` from each of those to reach the keys would
 * be a cycle.
 *
 * Hierarchical on purpose: `["spots"]` invalidates every spot query including
 * `["spots", id]` and `["spots", id, "host"]`, which is what a create, an edit or
 * a delete wants.
 */
export const viewKeys = {
  account: ["account"] as const,
  spots: ["spots"] as const,
  hostSpots: ["spots", "host"] as const,
  nearby: (lng: number, lat: number, meters: number) =>
    ["spots", "near", lng, lat, meters] as const,
  spot: (id: string) => ["spots", id] as const,
  hostSpot: (id: string) => ["spots", id, "host"] as const,
  bookings: ["bookings"] as const,
  renterBookings: ["bookings", "renter"] as const,
  nextBooking: ["bookings", "next"] as const,
  booking: (id: string) => ["bookings", id] as const,
  /** Every page of the wallet: `useInfiniteQuery` holds all its months under this. */
  wallet: ["wallet"] as const,
  balance: ["balance"] as const,
  /** Stripe Connect onboarding state. Answered live by payment-service, not projected. */
  connectAccount: ["connect", "account"] as const,
};
