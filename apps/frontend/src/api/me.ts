import { urqlClient, graphql } from "@/urql";
import { User } from "@/types/User";
import { ME } from "./graphql/user";

// Caller must set the access token first — authExchange reads it from the store.
export async function fetchMe(): Promise<User> {
  const { data } = await urqlClient
    .query(ME, {}, { url: graphql("user"), requestPolicy: "network-only" })
    .toPromise();

  return data.users[0];
}
