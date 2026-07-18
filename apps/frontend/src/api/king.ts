import { useAuthStore } from "@/stores/auth";
import { refreshAccessToken } from "./refresh";

// Paths are relative: the app is always served from the same origin that serves the
// API (Caddy in prod, the vite proxy in dev), which is what lets the refresh cookie
// ride along on the default `credentials: "same-origin"`.
async function baseFetch(path: string, options: RequestInit) {
  const auth = useAuthStore();

  const response = await fetch(path, {
    ...options,
    headers: {
      ...(auth.accessToken && {
        Authorization: `Bearer ${auth.accessToken}`,
      }),
      ...options.headers,
    },
  });

  return response;
}

export async function apiFetch(path: string, options: RequestInit = {}) {
  let res = await baseFetch(path, options);

  try {
    if (await isJwt401(res)) {
      const newToken = await refreshAccessToken();
      if (!newToken) return res; // refresh failed; already logged out + routed to login
      res = await baseFetch(path, options); // retry with the refreshed token
    }
  } catch (err: any) {
    console.log(err);
  }

  return res;
}

async function isJwt401(res: Response) {
  if (res.status !== 401) return false;

  try {
    const data = await res.json();
    // MyError bodies carry `detail` as a string array.
    return Array.isArray(data?.detail) && data.detail.includes("JWT");
  } catch {
    return false;
  }
}
