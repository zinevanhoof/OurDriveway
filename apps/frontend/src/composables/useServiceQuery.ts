import { useQuery, type UseQueryArgs } from "@urql/vue";
import { graphql, type Service } from "@/urql";

// Routes a query to one of the service GraphQL endpoints on the shared client.
// Everything else (auth, cache, fetch) is inherited from urqlClient.
export function useServiceQuery(service: Service, args: UseQueryArgs) {
  return useQuery({
    ...args,
    context: { url: graphql(service), ...args.context },
  });
}
