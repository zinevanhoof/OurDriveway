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
  query GetRentedBookings($renterId: String!) {
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

// "Earned this week" on the home screen, summed by the database rather than by
// shipping a week of bookings to add up here.
//
// `filter`, not `where`: the aggregate field is the one place that takes only the
// former — the list fields accept both. It always returns exactly one row, holding
// `{ amount_sum: 0 }` when nothing matches, so the caller reads [0] and never has
// an empty list to handle.
//
// `owner_id` is load-bearing, not a convenience. Aggregates respect table
// permissions, and booking's clause is `renter_id = $token.ID OR owner_id =
// $token.ID` — drop this and the sum quietly adds everything this person SPENT as
// a renter to what they EARNED as a host.
//
// Windowed on `created_at` (money booked this week), which with `owner_id` is what
// the booking_owner_created index exists for. No upper bound: nothing is created in
// the future, and one open end is a cleaner range scan.
const WEEK_EARNINGS = gql`
  query GetWeekEarnings($ownerId: String!, $since: datetime!) {
    bookings_aggregate(
      filter: {
        owner_id: { eq: $ownerId }
        status: { eq: "confirmed" }
        created_at: { gte: $since }
      }
    ) {
      amount_sum
    }
  }
`;

export { BOOKINGS_RENTED, WEEK_EARNINGS };
