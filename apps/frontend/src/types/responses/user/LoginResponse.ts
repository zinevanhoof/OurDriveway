/**
 * `POST /api/user/login`. `shared::responses::user::LoginResponse`.
 *
 * Same shape as {@link RefreshResponse} and still its own type: a response is named for
 * the request it answers.
 *
 * The `seq` that used to sit beside the token is gone from the body — it is the
 * `X-Version` response header now, recorded by the transport. The guarantee it exists for
 * is unchanged: the other half of a session, the refresh token, is an httponly cookie the
 * client cannot read, so without a version echoed back, `/refresh` could legitimately
 * read the projection before the session it is rotating has landed in it and answer 401.
 */
export type LoginResponse = {
  accessToken: string;
};
