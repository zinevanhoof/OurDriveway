import type { AccountUserResponse } from "./AccountUserResponse";

/**
 * `GET /api/view/account`.
 *
 * `user` is null only between signup and its projection. `id` comes from the verified
 * claim rather than a row, so it always resolves.
 */
export type AccountResponse = {
  id: string;
  user: AccountUserResponse | null;
};
