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

export { BOOKINGS_RENTED };
