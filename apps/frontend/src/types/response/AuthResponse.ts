export type AuthResponse = {
  access_token: string;
  /**
   * `SESSIONS:<n>` — where this session's event landed in the log. `userApi`
   * records it, so nothing that consumes an AuthResponse has to; it is typed here
   * because it is on the wire.
   */
  seq: string;
};
