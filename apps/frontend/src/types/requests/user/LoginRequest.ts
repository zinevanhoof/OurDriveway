/** `POST /api/user/login`. `shared::requests::user::LoginRequest`. */
export type LoginRequest = {
  email: string;
  password: string;
};
