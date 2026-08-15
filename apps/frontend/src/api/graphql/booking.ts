import { gql } from "@urql/vue";

// `spot` nests because the view projects a real `record<spot>` link, not just the
// denormalized `spot_id` string — one round trip instead of booking-then-spot.
// Nullable: a booking whose SpotCreated hasn't been projected yet has `spot: null`
// until the backfill lands, so every read of it must be optional.
//
// `timezone` is not decoration. `booked` holds bare wall-clock strings in the
// spot's zone, so without it "today", "upcoming" and "active now" would silently
// be answered in the *viewer's* zone instead — wrong for anyone booking abroad.
// `id` is what opens the detail drawer.
const BOOKINGS_RENTED = gql`
  query GetRentedBookings($renterId: uuid!) {
    bookings(where: { renter_id: { eq: $renterId } }) {
      id
      status
      amount
      booked
      # The last moment this booking occupies, folded server-side in the spot's
      # zone. Upcoming-vs-past is a comparison against it rather than a fold of
      # "booked" repeated in every client.
      endsAt: ends_at
      # 'spot_unavailable' when the host withdrew it — the renter is owed a refund
      # and an explanation, not a booking that silently disappeared.
      cancelReason: cancel_reason
      spot {
        id
        title
        images
        timezone
        address {
          line1
          city
          formatted
        }
      }
    }
  }
`;

// One booking's status, for the screen a payment redirect lands on.
//
// Just the status: that screen is waiting for a single transition and has no use for
// the spot, the slots or the price. `booking`'s select permission is
// `renter_id = $token.ID OR owner_id = $token.ID`, so this cannot be used to watch
// somebody else's booking — it returns null instead.
//
// There is deliberately no earnings aggregate in this file any more. Money figures come
// from payment-service, which owns them; a second total derived here would sooner or
// later disagree with the balance the withdraw button spends.
const BOOKING_STATUS = gql`
  query GetBookingStatus($id: ID!) {
    booking(id: $id) {
      id
      status
    }
  }
`;

export { BOOKINGS_RENTED, BOOKING_STATUS };
