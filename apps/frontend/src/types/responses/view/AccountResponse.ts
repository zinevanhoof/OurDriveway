import type { AccountProfileResponse } from "./AccountProfileResponse";

/**
 * `GET /api/view/account`.
 *
 * `profile` is null only between signup and its projection. `id` comes from the verified
 * claim rather than a row, so it always resolves.
 */
export type AccountResponse = {
  id: string;
  profile: AccountProfileResponse | null;
};
