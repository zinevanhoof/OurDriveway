import { Client, fetchExchange } from "@urql/vue";
import { cacheExchange } from "@urql/exchange-graphcache";
import { authExchange } from "@urql/exchange-auth";
import { useAuthStore } from "@/stores/auth";
import { refreshAccessToken } from "@/api/refresh";
import { awaitSeqHeader } from "@/lib/awaitSeq";

// Every read comes from the view service: one combined database, projected from
// all the event streams, with real record links so queries can nest. The
// per-service GraphQL endpoints still exist but nothing points at them.
export const GRAPHQL_URL = "/api/view/graphql";

export const urqlClient = new Client({
  url: GRAPHQL_URL,
  // urql defaults to GET-for-queries ("within-url-limit"); SurrealDB's GraphQL
  // endpoint only accepts POST with a JSON body, so force POST.
  preferGetMethod: false,
  // Reads are served from an eventually-consistent projection, so carry the
  // position of this client's newest write and let view-service block until its
  // projector has applied it. No-ops once the projector is past that position.
  fetchOptions: () => ({ headers: awaitSeqHeader() }),
  exchanges: [
    // `address`, `location` and `availability` are embedded on a spot (no own
    // id), so they can't be normalized. Return null to embed them on the parent
    // entity instead of keying them.
    //
    // `user` is the opposite case: a real record, so it is keyed by id and one
    // entry backs every reference to that person (a spot's owner, a booking's
    // renter, your own profile). The cost is that EVERY `user` selection set has
    // to include `id` — a keyable type selected without its key degrades
    // silently rather than erroring.
    cacheExchange({
      keys: {
        spot_address: () => null,
        GeometryPoint: () => null,
        spot_availability: () => null,
        user: (data) => data.id as string,
      },
    }),
    authExchange(async (utils) => ({
      addAuthToOperation(operation) {
        const token = useAuthStore().accessToken;
        return token
          ? utils.appendHeaders(operation, {
              Authorization: `Bearer ${token}`,
            })
          : operation;
      },
      // SurrealDB rejects an invalid/expired JWT; exact shape (HTTP 401 vs a
      // GraphQL error) isn't certain, so match both. Tune once you've seen the
      // real response from your endpoint.
      didAuthError(error) {
        return error.response?.status === 401;
      },
      // Shared with the REST layer: refreshes the token, or logs out + routes
      // to login on failure. authExchange retries the failed operation once.
      async refreshAuth() {
        await refreshAccessToken();
      },
    })),
    fetchExchange,
  ],
});
