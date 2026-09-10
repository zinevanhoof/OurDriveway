export type User = {
  /** "user:<uuid>" — the JWT claim, and what host_id/renter_id compare against. */
  id: string;
  firstName: string;
  lastName: string;
  profilePicture?: string | null;
};
