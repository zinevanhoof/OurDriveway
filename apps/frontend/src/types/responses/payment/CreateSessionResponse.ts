/** `POST /api/payment/session`. `shared::responses::payment::CreateSessionResponse`. */
export type CreateSessionResponse = {
  /** The only handle checkout carries in its URL. */
  sessionId: string;
  clientSecret: string;
};
