import { gql } from "@urql/vue";

// `owner` nests because the view projects a real `record<user>` link, not just the
// denormalized `owner_id` string — one round trip instead of spot-then-owner.
// Nullable: a spot whose UserRegistered hasn't been projected yet has `owner: null`
// until the backfill lands, so every read of it must be optional.
const FULL_SPOT = gql`
  query GetSpot($id: ID!) {
    spot(id: $id) {
      id
      owner {
        # Not read by the template — it is the graphcache key. A user selected
        # without its id can't normalize, and does so quietly.
        id
        firstName: first_name
        lastName: last_name
        profilePicture: profile_picture
      }
      title
      description
      price_per_hour
      images
      timezone
      address {
        formatted
      }
      availability {
        weekly
        single
      }
      # Slots already taken: "YYYY-MM-DD" -> [{ start, end }]. Same shape as
      # availability.single, so the picker subtracts one from the other directly.
      # This replaced a spot_busy query — and before that a booking query that
      # was a live bug, since a prospective renter can't select booking rows at all.
      booked
    }
  }
`;

const MANAGE_SPOT = gql`
  # "datetime" (lowercase) is SurrealDB's own scalar name for a datetime field —
  # it coerces an RFC 3339 string, which is what toISOString() gives.
  query GetSpotAndBookings($spotId: ID!, $now: datetime!) {
    spot(id: $spotId) {
      id
      title
      description
      pricePerHour: price_per_hour
      images
      timezone
      # Drives the live switch. Not the same as "deleted" — off just stops new
      # reservations, and the bookings already taken are still honoured.
      active
      address {
        formatted
      }
      availability {
        weekly
        single
      }
      # Slots already taken: "YYYY-MM-DD" -> [{ start, end }]. Same shape as
      # availability.single, so the picker subtracts one from the other directly.
      # This replaced a spot_busy query — and before that a booking query that
      # was a live bug, since a prospective renter can't select booking rows at all.
      booked
    }
    # Still to come, as one indexed comparison. The alternative is folding every
    # booking's date map client-side, which a GraphQL filter cannot express — hence
    # ends_at existing at all.
    bookings(where: { spot_id: { eq: $spotId }, ends_at: { gt: $now } }) {
      id
      renter {
        id
        firstName: first_name
        lastName: last_name
        profilePicture: profile_picture
      }
      booked
      amount
      status
      endsAt: ends_at
    }
  }
`;

const EDIT_SPOT = gql`
  query GetSpot($id: ID!) {
    spot(id: $id) {
      id
      title
      description
      pricePerHour: price_per_hour
      images
      # The zone the bare dates in "booked" are relative to — without it the edit
      # screen can't tell which of them are still in the future.
      timezone
      address {
        line1
        line2
        city
        postalCode: postal_code
        region
        country
      }
      availability {
        weekly
        single
      }
      # Slots already taken: "YYYY-MM-DD" -> [{ start, end }]. Same shape as
      # availability.single, so the picker subtracts one from the other directly.
      # This replaced a spot_busy query — and before that a booking query that
      # was a live bug, since a prospective renter can't select booking rows at all.
      booked
    }
  }
`;

const SPOT = gql`
  query GetSpot($id: ID!) {
    spot(id: $id) {
      id
      title
      address {
        formatted
      }
    }
  }
`;

const SPOTS_OWNED = gql`
  # The owner can select their own inactive spots — that's what the switch is for —
  # so the list has to exclude deleted ones itself. The row survives only so a
  # renter's past bookings can still resolve a title and an address.
  query GetOwnedSpots($id: String!) {
    spots(where: { owner_id: { eq: $id }, deleted: { eq: false } }) {
      id
      title
      images
      price_per_hour
      active
      address {
        line1
        city
      }
    }
  }
`;

// Spots within `meters` of [lng, lat] (map center). SurrealDB's auto GraphQL can't
// bbox a geometry<point>, but the `call` operator runs a SurrealQL fn on the field:
// fn::spot_distance(location, lng, lat) < meters — a custom fn (in spot-schema.surql)
// that rebuilds the point from plain floats, since geo::distance's geometry arg can't
// pass through the JSON filter. `op: lt` is a fixed enum literal (enums can't be
// variables); the coords/radius ride the `JSON` scalar the filter uses.
//
// `availability` (weekly + single, both `object` scalars) is selected so the map's
// weekday/date + time-slot filter can be applied client-side (see spotMatches). The
// auto GraphQL filter input excludes object fields, so it can't be a `where` clause.
const SPOTS_IN_RADIUS = gql`
  query SpotsInRadius($lng: Float!, $lat: Float!, $meters: Float!) {
    spots(
      where: {
        # Table permissions already hide other people's inactive spots, but not the
        # viewer's own — without this the host keeps seeing a listing they deleted.
        deleted: { eq: false }
        active: { eq: true }
        location: {
          call: {
            fn: "fn::spot_distance"
            args: [$lng, $lat]
            op: lt
            value: $meters
          }
        }
      }
    ) {
      id
      title
      price_per_hour
      location {
        coordinates
      }
      availability {
        weekly
        single
      }
    }
  }
`;

export {
  FULL_SPOT,
  MANAGE_SPOT,
  EDIT_SPOT,
  SPOT,
  SPOTS_OWNED,
  SPOTS_IN_RADIUS,
};
