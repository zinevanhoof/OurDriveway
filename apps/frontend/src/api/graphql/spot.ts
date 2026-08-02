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
  query GetOwnedSpots($id: String!) {
    spots(where: { owner_id: { eq: $id } }) {
      id
      title
      images
      price_per_hour
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

export { FULL_SPOT, SPOT, SPOTS_OWNED, SPOTS_IN_RADIUS };
