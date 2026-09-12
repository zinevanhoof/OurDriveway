/**
 * `POST /api/user/session/refresh`. `shared::responses::user::RefreshResponse`.
 *
 * No request body — the refresh token rides in the httponly cookie.
 */
export type RefreshResponse = {
  accessToken: string;
};
