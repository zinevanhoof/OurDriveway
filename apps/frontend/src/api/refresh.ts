import { useAuthStore } from "@/stores/auth";
import { refreshUser } from "./userApi";

// Shared, deduped access-token refresh used by both the REST layer (king.ts)
// and the GraphQL client (urql authExchange). Concurrent callers await the same
// in-flight refresh instead of each hitting the refresh route.
let refreshPromise: Promise<string | null> | null = null;

export function refreshAccessToken(): Promise<string | null> {
  if (refreshPromise) return refreshPromise;

  refreshPromise = (async () => {
    const auth = useAuthStore();
    try {
      const res = await refreshUser();
      if (!res.ok) throw new Error("refresh failed");

      const { access_token } = await res.json();
      auth.setAccessToken(access_token);
      return access_token as string;
    } catch {
      // Refresh token gone/expired — end the session. main.ts watches
      // isAuthenticated and handles routing to login.
      auth.logout();
      return null;
    } finally {
      refreshPromise = null;
    }
  })();

  return refreshPromise;
}
