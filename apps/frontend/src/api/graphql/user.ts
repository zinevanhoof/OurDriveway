import { gql } from "@urql/vue";

const ME = gql`
  query Me {
    users {
      id
      email
    }
  }
`;

export { ME };
