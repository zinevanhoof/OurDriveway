import { z } from "zod";

/**
 * The password rules, in one place because two forms enforce them: signup and
 * change-password.
 *
 * These messages are mirrored verbatim by the garde validators in
 * `shared/src/requests/user.rs`, which has a test asserting the two schemas
 * agree. Change a rule here and change it there, or the form passes and the
 * request 422s with wording the user has never seen.
 */
export const passwordRules = z
  .string()
  .min(8, "Password must be at least 8 characters")
  .max(32, "Password must be at most 32 characters")
  .regex(/[A-Z]/, "Must contain at least one uppercase letter")
  .regex(/[a-z]/, "Must contain at least one lowercase letter")
  .regex(/[0-9]/, "Must contain at least one number")
  .regex(/[^A-Za-z0-9]/, "Must contain at least one special character");
