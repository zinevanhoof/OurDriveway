/**
 * `PATCH /api/user`. `shared::requests::user::UpdateUserRequest`.
 *
 * Every field is optional server-side, where omitted means "unchanged" — which is what
 * lets one endpoint serve two screens, each sending only its own half.
 *
 * The two halves are separate types below rather than one optional-everything type,
 * because each screen really does submit its whole half, and **the server refuses a body
 * carrying both**: it publishes one event per request, and a password change is a
 * different event from a profile edit. Sending both is a 409, not a merge.
 */
export type UpdateUserRequest = {
  firstName?: string;
  lastName?: string;
  email?: string;
  licensePlates?: string[];
  country?: string;
  currentPassword?: string;
  newPassword?: string;
  profilePicture?: string;
};

/** The edit-profile screen's half. */
export type UpdateProfileRequest = {
  firstName: string;
  lastName: string;
  email: string;
  licensePlates: string[];
  /**
   * ISO 3166-1 alpha-2, or omitted for "unchanged".
   *
   * Where the user banks, not where they live — it is what Stripe opens their payout
   * account with, and it cannot be changed once that account exists.
   */
  country?: string;
  /** Only required when `email` differs from the stored one; the server decides. */
  currentPassword?: string;
  /**
   * A media URL under `avatars/`, or omitted for "unchanged" — which is what most saves
   * send, since the picture only changes when the user picks a new one. There is no way
   * to express "remove", because there is no UI for it.
   */
  profilePicture?: string;
};

/** The change-password screen's half. */
export type ChangePasswordRequest = {
  currentPassword: string;
  newPassword: string;
};
