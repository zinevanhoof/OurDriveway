/**
 * `PATCH /api/user`. `shared::requests::user::UpdateUserRequest`.
 *
 * Every field is optional, here as on the server, where omitted means "unchanged" —
 * which is what lets one endpoint serve three callers: the edit screen sending its whole
 * half, the password screen sending nothing but the two password fields, and the booking
 * form sending one plate a renter typed on it.
 *
 * There used to be an `UpdateProfileRequest` beside this, spelling the edit screen's half
 * with its fields required. It said something true about that screen, in the one place
 * that is about the *endpoint* — and it left the word "profile" naming a request body when
 * nothing on the server has ever called it that. That screen still submits every field it
 * shows; it is a fact about the form, not about the wire.
 *
 * **The server refuses a body carrying both halves**: it publishes one event per request,
 * and a password change is a different event from an edit. Sending both is a 409, not a
 * merge.
 */
export type UpdateUserRequest = {
  firstName?: string;
  lastName?: string;
  email?: string;
  licensePlates?: string[];
  /**
   * ISO 3166-1 alpha-2, or omitted for "unchanged".
   *
   * Where the user banks, not where they live — it is what Stripe opens their payout
   * account with, and it cannot be changed once that account exists.
   */
  country?: string;
  /**
   * Required for a password change, and for an email change; the server decides which,
   * because only it knows the stored address.
   */
  currentPassword?: string;
  newPassword?: string;
  /**
   * A media URL under `avatars/`, or omitted for "unchanged" — which is what most saves
   * send, since the picture only changes when the user picks a new one. There is no way
   * to express "remove", because there is no UI for it.
   */
  profilePicture?: string;
};

/** The change-password screen's half, where both fields really are required. */
export type ChangePasswordRequest = {
  currentPassword: string;
  newPassword: string;
};
