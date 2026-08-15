import { gql } from "@urql/vue";

// A host's withdrawal history.
//
// Only payouts live in the read model — what a renter was charged and what a host has
// available come from payment-service, which owns them. Duplicating the figures here
// would give the withdraw button one total and the panel around it another.
//
// `owner_id` is load-bearing rather than a convenience: `payout`'s select permission is
// `owner_id = $token.ID`, so this could not return anyone else's rows, but naming it
// lets the query use the `payout_owner` index instead of scanning and filtering.
const PAYOUTS = gql`
  query GetPayouts($ownerId: uuid!) {
    payouts(where: { owner_id: { eq: $ownerId } }) {
      id
      amount
      createdAt: created_at
    }
  }
`;

export { PAYOUTS };
