import { gql } from "@urql/vue";

// `owner` nests because the view projects a real `record<user>` link, not just the
// denormalized `owner_id` string — one round trip instead of spot-then-owner.
// Nullable: a spot whose UserRegistered hasn't been projected yet has `owner: null`
// until the backfill lands, so every read of it must be optional.
// Two spellings of the same spot, deliberately, and a clock: spot(id:) is a record
// LOOKUP taking the u'<uuid>' literal (gqlRecordId()), spot_id is a TYPE uuid FIELD
// whose eq takes the plain uuid (plainUuid()), and $now floors the booking list.
// Passing either spelling to the other silently returns nothing.
const FULL_SPOT = gql`
  query GetSpot($id: ID!, $spotUuid: uuid!, $now: datetime!) {
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
    }
    # What is already taken, straight off the bookings. No status filter: the view's
    # booking select permission admits released and cancelled rows only to the renter
    # or the owner, so what a prospective renter sees here is exactly what blocks a
    # slot. mergeBooked() unions these into one "YYYY-MM-DD" -> slots map, the same
    # shape as availability.single, and the picker subtracts one from the other.
    bookings(where: { spot_id: { eq: $spotUuid }, ends_at: { gt: $now } }) {
      booked
    }
  }
`;

const MANAGE_SPOT = gql`
  # "datetime" (lowercase) is SurrealDB's own scalar name for a datetime field —
  # it coerces an RFC 3339 string, which is what toISOString() gives.
  # Two spellings of the same spot, deliberately: spot(id:) is a record LOOKUP and
  # takes the u'<uuid>' literal (gqlRecordId()), while spot_id is a TYPE uuid FIELD
  # whose eq takes the plain uuid (plainUuid()). Passing either one to the other
  # silently returns nothing — hence two variables rather than one reused.
  query GetSpotAndBookings($spotId: ID!, $spotUuid: uuid!, $now: datetime!) {
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
    }
    # Still to come, as one indexed comparison. The alternative is folding every
    # booking's date map client-side, which a GraphQL filter cannot express — hence
    # ends_at existing at all.
    bookings(where: { spot_id: { eq: $spotUuid }, ends_at: { gt: $now } }) {
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
  query GetSpot($id: ID!, $spotUuid: uuid!, $now: datetime!) {
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
    }
    # The bookings the edit screen warns about: narrowing the hours cancels any of
    # these that fall outside them. Same shape and the same reasoning as FULL_SPOT.
    bookings(where: { spot_id: { eq: $spotUuid }, ends_at: { gt: $now } }) {
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
  query GetOwnedSpots($id: uuid!) {
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

// The home screen's two "nearby spots" cards. Same `call` filter as
// SPOTS_IN_RADIUS, but a separate document on purpose: that one refires on every
// debounced map pan, and widening it would put every spot's `images` array on the
// wire each time to serve a screen that draws two cards once. This one drops
// `availability` in return (nothing filters client-side here). graphcache keys
// `spot` by id, so both still share the same cached entities.
//
// `owner_id: { ne }` server-side rather than a filter here — "somewhere to park"
// never means your own driveway, and excluding it client-side could leave one card.
//
// No ordering by distance: auto GraphQL's `order` argument takes an enum of defined
// field names, so it cannot order by a function at all. The caller sorts the handful
// this returns with `nearer()` and keeps two.
const SPOTS_NEARBY = gql`
  query SpotsNearby($lng: Float!, $lat: Float!, $meters: Float!, $me: uuid!) {
    spots(
      where: {
        deleted: { eq: false }
        active: { eq: true }
        owner_id: { ne: $me }
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
      images
      location {
        coordinates
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
  SPOTS_NEARBY,
};
