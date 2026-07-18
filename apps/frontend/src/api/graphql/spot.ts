import { gql } from "@urql/vue";

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
      address {
        formatted
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
      location {
        coordinates
      }
    }
  }
`;

export { SPOT, SPOTS_OWNED, SPOTS_IN_RADIUS };
