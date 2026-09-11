/**
 * `POST /api/payment/session`. `shared::requests::payment::CreateSessionRequest`.
 *
 * The amount is never sent: the server takes it from the booking as it priced it at
 * reserve time, so the figure on screen is display and the figure charged is the
 * server's.
 */
export type CreateSessionRequest = {
  bookingId: string;
  /** Decided client-side — only this side knows whether it is a browser or Tauri. */
  returnUrl: string;
};
