import { useAuthStore } from "@/stores/auth";
import { refreshSession } from "./userApi";

// Shared, deduped access-token refresh, used by the transport's 401 path and by
// `main.ts` at startup. Concurrent callers await the same in-flight refresh
// instead of each hitting the refresh route.
//
// (It used to say "and the GraphQL client (urql authExchange)". There is no urql.)
let refreshPromise: Promise<string | null> | null = null;

export function refreshAccessToken(): Promise<string | null> {
  if (refreshPromise) return refreshPromise;

  refreshPromise = (async () => {
    const auth = useAuthStore();
    try {
      const { accessToken } = await refreshSession();
      auth.setAccessToken(accessToken);
      return accessToken;
    } catch {
      // Refresh token gone or expired — end the session. `main.ts` watches
      // `isAuthenticated` and handles routing to login.
      auth.logout();
      return null;
    } finally {
      refreshPromise = null;
    }
  })();

  return refreshPromise;
}
