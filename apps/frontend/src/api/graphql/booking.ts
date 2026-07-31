import { gql } from "@urql/vue";

// Occupied slots for a spot, as projected into `spot_busy`.
//
// This replaced a query against `booking` filtered by spot_id. That query was a
// live bug: `booking` is only selectable by its renter or the spot owner, so a
// prospective renter always got an empty list back and every slot looked free.
// `spot_busy` carries no renter identity, so it is safe to read publicly.
const SPOT_BUSY = gql`
  query SpotBusy($id: String!) {
    spotBusies(where: { spot_id: { eq: $id } }) {
      date
      slots {
        start
        end
      }
    }
  }
`;

export { SPOT_BUSY };
