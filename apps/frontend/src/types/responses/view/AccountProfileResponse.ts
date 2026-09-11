/**
 * The caller's own profile — the one place `email`, `licensePlates` and `country` appear.
 *
 * `email` is not nullable: every projected row is created by a registration, which carries
 * one, and the column is `NOT NULL`.
 */
export type AccountProfileResponse = {
  firstName: string;
  lastName: string;
  profilePicture: string | null;
  email: string;
  licensePlates: string[];
  /**
   * ISO 3166-1 alpha-2, or null until the profile sets it.
   *
   * Scoped like `email` rather than like `licensePlates`: only its owner sees it. It
   * exists because Stripe will not open a connected account without a country and fixes it
   * permanently at creation, so a host sets it once, before onboarding.
   */
  country: string | null;
};
