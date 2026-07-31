export type User = {
  /** "user:<uuid>" — the JWT claim, and what owner_id/renter_id compare against. */
  id: string;
  firstName: string;
  lastName: string;
  profilePicture?: string | null;
};
