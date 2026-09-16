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
  /** Every page of the host's listings: `useInfiniteQuery` holds them all under this. */
  hostSpots: ["spots", "host"] as const,
  nearby: (lng: number, lat: number, meters: number) =>
    ["spots", "near", lng, lat, meters] as const,
  spot: (id: string) => ["spots", id] as const,
  /** A spot the caller booked. Under `["spots"]`, so a listing edit refreshes it too. */
  renterSpot: (id: string) => ["spots", id, "renter"] as const,
  hostSpot: (id: string) => ["spots", id, "host"] as const,
  /** The host's merged taken-slot map, for the edit form's warning. */
  hostSpotBooked: (id: string) => ["spots", id, "host", "booked"] as const,
  /**
   * One tab of one spot's paged bookings. Under `hostSpot`'s key on purpose: a booking
   * landing invalidates `["spots"]`, and this has to go with it.
   */
  hostSpotBookings: (id: string, scope: string, status: string) =>
    ["spots", id, "host", "bookings", scope, status] as const,
  /**
   * The manage screen's two-row preview. Its own key rather than a `hostSpotBookings`
   * one: that is an infinite query, which caches `{ pages }` rather than one response.
   */
  hostSpotBookingsPreview: (id: string) =>
    ["spots", id, "host", "bookings", "preview"] as const,
  bookings: ["bookings"] as const,
  /**
   * The taken slots on a public spot. Under `["bookings"]` rather than the spot's key: a
   * booking landing is what changes them, and that invalidates `["bookings"]`.
   */
  spotBookings: (id: string) => ["bookings", "spot", id] as const,
  /** Every page of one tab of the renter's own bookings. */
  renterBookings: (scope: string) => ["bookings", "renter", scope] as const,
  nextBooking: ["bookings", "next"] as const,
  booking: (id: string) => ["bookings", id] as const,
  /** Every page of the wallet: `useInfiniteQuery` holds all its months under this. */
  wallet: ["wallet"] as const,
  balance: ["balance"] as const,
  /** Stripe Connect onboarding state. Answered live by payment-service, not projected. */
  connectAccount: ["connect", "account"] as const,
};
