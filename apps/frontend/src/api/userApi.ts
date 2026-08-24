import { apiFetch } from "./king";
import { recordSeq } from "@/lib/awaitSeq";
import { LoginRequest } from "@/types/requests/LoginRequest";
import { SignupRequest } from "@/types/requests/SignupRequest";

// Every write below runs through `record`, including the three that answer with
// something other than a 202: login and refresh carry a SESSIONS position on the
// AuthResponse, and signup and verify answer 202 like the rest. user-service now
// mounts the await_seq layer, so those positions are what make the *next* call to
// it — a second signup submit, the login after a verification — read what this one
// just wrote, on whichever replica takes it.
const loginUser = async ({
  email,
  password,
}: LoginRequest): Promise<Response> =>
  record(
    await apiFetch("/api/user/login", {
      method: "POST",
      body: JSON.stringify({
        email,
        password,
      }),
      headers: {
        "Content-Type": "application/json",
      },
    }),
  );

const signupUser = async ({
  firstName,
  lastName,
  email,
  password,
}: SignupRequest): Promise<Response> =>
  record(
    await apiFetch("/api/user/signup", {
      method: "POST",
      body: JSON.stringify({
        firstName,
        lastName,
        email,
        password,
      }),
      headers: {
        "Content-Type": "application/json",
      },
    }),
  );

/**
 * Confirms an address from the token in a mailed link.
 *
 * A POST, not a GET, even though it is reached by clicking a link: mail scanners
 * prefetch links, so the link itself goes to a page and only this call has an
 * effect. Safe to run twice — the backend treats re-verification as a no-op
 * rather than an error, which is what makes a prefetch harmless.
 */
const verifyEmail = async (token: string): Promise<Response> =>
  record(
    await apiFetch("/api/user/email/verify", {
      method: "POST",
      body: JSON.stringify({ token }),
      headers: { "Content-Type": "application/json" },
    }),
  );

/**
 * Asks for the verification link again.
 *
 * Always 204, even for an address with no account — the backend refuses to say
 * which addresses are registered, so there is nothing here to branch on.
 */
const resendVerification = async (email: string): Promise<Response> =>
  apiFetch("/api/user/email/resend", {
    method: "POST",
    body: JSON.stringify({ email }),
    headers: { "Content-Type": "application/json" },
  });

const logoutUser = async () => {
  await apiFetch("/api/user/session/logout", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  });
};

const refreshUser = async (): Promise<Response> =>
  record(
    await apiFetch("/api/user/session/refresh", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
    }),
  );

/**
 * The two halves of `PATCH /api/user`. Every field is optional server-side, where
 * omitted means "unchanged" — which is what lets one endpoint serve both screens,
 * each sending only its own half.
 *
 * They are two types here rather than one optional-everything type because each
 * screen really does submit its whole half, and the server refuses a body carrying
 * both: it publishes one event per request, and a password change is a different
 * event from a profile edit.
 */
export type UpdateProfileRequest = {
  firstName: string;
  lastName: string;
  email: string;
  licensePlates: string[];
  /** Only required when `email` differs from the stored one; the server decides. */
  currentPassword?: string;
  /**
   * A media key under `avatars/`, or omitted for "unchanged" — which is what most
   * saves send, since the picture only changes when the user picks a new one.
   * There is no way to express "remove", because there is no UI for it.
   */
  profilePicture?: string;
};

export type ChangePasswordRequest = {
  currentPassword: string;
  newPassword: string;
};

/**
 * Saves the edit-profile form. Returns the raw Response, like the rest of this
 * file, so the form can map a 422's `errors` map back onto its own fields.
 *
 * Answers 202 with `{ seq }`: the event is in the log, but the projection that
 * answers reads is still catching up, and recording the seq is what makes the
 * next query wait for this write.
 */
const updateProfile = (body: UpdateProfileRequest): Promise<Response> =>
  patchUser(body);

/** Same endpoint, the other half — see [UpdateProfileRequest]. */
const changePassword = (body: ChangePasswordRequest): Promise<Response> =>
  patchUser(body);

const patchUser = async (
  body: UpdateProfileRequest | ChangePasswordRequest,
): Promise<Response> =>
  record(
    await apiFetch("/api/user", {
      method: "PATCH",
      body: JSON.stringify(body),
      headers: { "Content-Type": "application/json" },
    }),
  );

/** Captures the log position from a 202 without consuming the caller's body. */
async function record(response: Response): Promise<Response> {
  if (response.ok) {
    const { seq } = await response
      .clone()
      .json()
      .catch(() => ({ seq: null }));
    recordSeq(seq);
  }
  return response;
}

export {
  loginUser,
  signupUser,
  logoutUser,
  refreshUser,
  updateProfile,
  changePassword,
  verifyEmail,
  resendVerification,
};
