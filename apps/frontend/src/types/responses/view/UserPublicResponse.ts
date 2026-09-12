/** A person as anyone may see them. Never carries an email. */
export type UserPublicResponse = {
  id: string;
  firstName: string;
  lastName: string;
  profilePicture: string | null;
};
