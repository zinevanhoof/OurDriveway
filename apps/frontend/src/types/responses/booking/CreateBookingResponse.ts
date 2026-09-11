/**
 * `POST /api/booking`. `shared::responses::booking::CreateBookingResponse`.
 *
 * The one write that answers with more than a version. The id has to come back here —
 * the booking is minted server-side and the client opens a Stripe Checkout Session from
 * it straight away, before any projection has caught up.
 */
export type CreateBookingResponse = {
  id: string;
};
