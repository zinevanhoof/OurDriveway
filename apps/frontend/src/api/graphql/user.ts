import { gql } from "@urql/vue";

// `email` resolves only for the user asking: the view's `user` table is
// world-readable by design (spot-owner profiles have to resolve for everyone),
// so the address is scoped at FIELD level in view-schema.surql and comes back
// null for anybody else. Selecting it on another user is not an error, just
// empty — which is why this query is only ever run with your own id.
const ME = gql`
  query GetMyself($id: ID!) {
    user(id: $id) {
      id
      firstName: first_name
      lastName: last_name
      profilePicture: profile_picture
      email
      licensePlates: license_plates
    }
  }
`;

export { ME };
