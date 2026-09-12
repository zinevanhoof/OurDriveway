/** `POST /api/user/signup`. `shared::requests::user::SignupRequest`. */
export type SignupRequest = {
  firstName: string;
  lastName: string;
  email: string;
  password: string;
};
