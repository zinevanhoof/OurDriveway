/**
 * `POST /api/payment/connect/session`.
 * `shared::responses::payment::AccountSessionResponse`.
 *
 * A client secret for Connect's embedded components, creating the caller's connected
 * account on the first call — which is why it is a POST rather than a read.
 */
export type AccountSessionResponse = {
  clientSecret: string;
};
