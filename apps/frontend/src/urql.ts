import { Client, fetchExchange } from "@urql/vue";
import { cacheExchange } from "@urql/exchange-graphcache";
import { authExchange } from "@urql/exchange-auth";
import { useAuthStore } from "@/stores/auth";
import { refreshAccessToken } from "@/api/refresh";

export type Service = "user" | "booking" | "spot";

// Each service has its own GraphQL endpoint. One client routes per operation
// via context.url (see useServiceQuery), falling back to this default.
export const graphql = (s: Service) => `/api/${s}/graphql`;

export const urqlClient = new Client({
  url: graphql("spot"),
  // urql defaults to GET-for-queries ("within-url-limit"); SurrealDB's GraphQL
  // endpoint only accepts POST with a JSON body, so force POST.
  preferGetMethod: false,
  exchanges: [
    // `address` is embedded on a spot (no own id), so it can't be normalized.
    // Return null to embed it on the parent entity instead of keying it.
    cacheExchange({
      keys: { spot_address: () => null },
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
