/**
 * `GET /api/view/account/notifications` — one open notification.
 * `shared::responses::view::NotificationResponse`.
 *
 * A union on `kind`: a new kind is a new member here and a new branch in
 * `NotificationsView`.
 */
export type NotificationResponse = {
  /** When it became visible — a rating prompt when the booking ended, the rest when they happened. */
  at: string;
  /** Visible since before the list was last opened. The badge counts the unseen ones. */
  seen: boolean;
} & (
  | {
      kind: "rate_booking";
      bookingId: string;
      /** `null` when the spot had not reached the read model yet at confirmation. */
      spotTitle: string | null;
    }
  | {
      /** A renter paid for a booking on the caller's spot. Dismissed by tapping it. */
      kind: "spot_booked";
      bookingId: string;
      spotId: string;
      spotTitle: string | null;
      /** The renter's first name, `null` when their user row had not been projected. */
      renterName: string | null;
    }
);
