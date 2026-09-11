import { useMutation, useQueryClient } from "@tanstack/vue-query";

import { patch, post } from "./client";
import { viewKeys } from "./keys";
import type { LoginRequest } from "@/types/requests/user/LoginRequest";
import type { SignupRequest } from "@/types/requests/user/SignupRequest";
import type {
  ChangePasswordRequest,
  UpdateProfileRequest,
} from "@/types/requests/user/UpdateUserRequest";
import type { LoginResponse } from "@/types/responses/user/LoginResponse";
import type { RefreshResponse } from "@/types/responses/user/RefreshResponse";

// Every function here used to return a raw `Response` and leave the component to
// branch on its status. They return their parsed body or throw an `ApiError` now,
// like the rest of the api layer.
//
// Nothing calls `recordVersion`. The version each of these writes reaches arrives as
// the `X-Version` header and is recorded by the transport — including on login
// and refresh, which carry the session's version so the *next* call to user-service
// reads what this one just wrote, on whichever replica takes it.

/**
 * Exchanges credentials for an access token, and sets the refresh cookie.
 *
 * A wrong password is a 401 with `detail: ["Invalid credentials"]`, and it reaches
 * the caller intact — which it did not before. `king.ts` read a 401's body to
 * decide whether to refresh, without cloning, so the form's own read of it threw
 * and every wrong password rendered as "Something went wrong. Please try again."
 * See the note at the top of `client.ts`.
 *
 * An unverified address is a **403**, which is a different conversation: the fix is
 * a link in their inbox, not another guess.
 */
export const login = (body: LoginRequest) =>
  post<LoginResponse>("/api/user/login", body);

export const signup = (body: SignupRequest) =>
  post<void>("/api/user/signup", body);

/**
 * Confirms an address from the token in a mailed link.
 *
 * A POST, not a GET, even though it is reached by clicking a link: mail scanners
 * prefetch links, so the link itself goes to a page and only this call has an
 * effect. Safe to run twice — the backend treats re-verification as a no-op rather
 * than an error, which is what makes a prefetch harmless.
 */
export const verifyEmail = (token: string) =>
  post<void>("/api/user/email/verify", { token });

/**
 * Asks for the verification link again.
 *
 * Always 204, even for an address with no account — the backend refuses to say
 * which addresses are registered, so there is nothing here to branch on.
 */
export const resendVerification = (email: string) =>
  post<void>("/api/user/email/resend", { email });

export const logout = () => post<void>("/api/user/session/logout");

/**
 * Rotates the session from the httponly refresh cookie.
 *
 * Not a hook and never will be: `refresh.ts` calls it from inside the transport's
 * own 401 path, and `main.ts` calls it before the app — and its QueryClient —
 * exists.
 */
export const refreshSession = () =>
  post<RefreshResponse>("/api/user/session/refresh");

/**
 * The two halves of `PATCH /api/user`. Every field is optional server-side, where
 * omitted means "unchanged" — which is what lets one endpoint serve both screens,
 * each sending only its own half. The server refuses a body carrying both, with a
 * 409: it publishes one event per request, and a password change is a different
 * event from a profile edit.
 */
export const updateProfile = (body: UpdateProfileRequest) =>
  patch<void>("/api/user", body);

/** Same endpoint, the other half. */
export const changePassword = (body: ChangePasswordRequest) =>
  patch<void>("/api/user", body);

// ─── hooks ──────────────────────────────────────────────────────────────────
//
// The invalidations below used to live in the components, next to the `await`
// rather than next to the write — which is the pair that drifts. A write knows
// what it changed; the screen that happened to trigger it does not.

export const useLogin = () => useMutation({ mutationFn: login });

export const useSignup = () => useMutation({ mutationFn: signup });

export const useVerifyEmail = () => useMutation({ mutationFn: verifyEmail });

export const useResendVerification = () =>
  useMutation({ mutationFn: resendVerification });

export function useLogout() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: logout,
    // Every cached read belongs to the session that just ended. `clear` rather
    // than `invalidateQueries`, which would refetch them all as the anonymous
    // user on the way out.
    onSettled: () => queryClient.clear(),
  });
}

export function useUpdateProfile() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: updateProfile,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: viewKeys.account }),
  });
}

export const useChangePassword = () =>
  useMutation({ mutationFn: changePassword });
