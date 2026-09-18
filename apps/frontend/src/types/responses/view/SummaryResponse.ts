/**
 * `GET /api/view/public/users/{id}/summary` — a person's reputation as a host.
 * Hide each figure that is zero.
 */
export type UserSummaryResponse = {
  /** Confirmed bookings on their spots that are over. */
  bookings: number;
  /** The average rating they have received, or null when nobody has rated them. */
  rating: number | null;
  ratings: number;
};

/** `GET /api/view/public/spots/{id}/summary` — a spot's rating, for anyone. */
export type SpotSummaryResponse = {
  /** The average rating, or null when nobody has rated this spot. */
  rating: number | null;
  ratings: number;
};

/**
 * `GET /api/view/host/summary` — the caller's totals as a host: the profile row and the
 * tiles on top of "Your parking spots".
 */
export type HostSummaryResponse = {
  /** Listings that are not deleted, paused ones included. */
  spots: number;
  /** Of those, the ones that are live. */
  activeSpots: number;
  /** Listings with a confirmed booking happening right now, on each spot's clock. */
  bookedNow: number;
  /** Confirmed bookings across all their spots that are over. */
  bookings: number;
  /** EUR cents. Paid bookings still confirmed, upcoming ones included. */
  earnedCents: number;
  /** EUR cents. The same, for payments made this calendar month (UTC). */
  earnedThisMonthCents: number;
  /** EUR cents. The same, for last calendar month. */
  earnedLastMonthCents: number;
};

/** `GET /api/view/host/spots/{id}/summary` — how one of the host's spots is doing. */
export type HostSpotSummaryResponse = {
  /** Confirmed bookings on this spot that are over. */
  bookings: number;
  /** EUR cents. Paid bookings still confirmed, upcoming ones included. */
  earnedCents: number;
  /** The average rating, or null when nobody has rated this spot. */
  rating: number | null;
  ratings: number;
};
