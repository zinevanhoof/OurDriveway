import { apiFetch } from "./king";
import { recordSeq } from "@/lib/awaitSeq";
import { LoginRequest } from "@/types/requests/LoginRequest";
import { SignupRequest } from "@/types/requests/SignupRequest";

const loginUser = async ({
  email,
  password,
}: LoginRequest): Promise<Response> => {
  const response = await apiFetch("/api/user/login", {
    method: "POST",
    body: JSON.stringify({
      email,
      password,
    }),
    headers: {
      "Content-Type": "application/json",
    },
  });

  return response;
};

const signupUser = async ({
  firstName,
  lastName,
  email,
  password,
}: SignupRequest): Promise<Response> => {
  const response = await apiFetch("/api/user/signup", {
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
  });

  return response;
};

const logoutUser = async () => {
  await apiFetch("/api/user/refresh/logout", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  });
};

const refreshUser = async (): Promise<Response> => {
  const response = await apiFetch("/api/user/refresh", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  });

  return response;
};

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
const updateProfile = async (body: UpdateProfileRequest): Promise<Response> =>
  record(
    await apiFetch("/api/user/me", {
      method: "PATCH",
      body: JSON.stringify(body),
      headers: { "Content-Type": "application/json" },
    }),
  );

const changePassword = async (body: ChangePasswordRequest): Promise<Response> =>
  record(
    await apiFetch("/api/user/me/password", {
      method: "POST",
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
};
