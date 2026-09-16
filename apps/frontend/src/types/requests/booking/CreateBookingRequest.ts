import type { SingleAvailability } from "@/types/domain/spot";

/**
 * `POST /api/booking`. `shared::requests::booking::CreateBookingRequest`.
 *
 * This was `BookingDraft`, in a file called `BookingRequest.ts` that exported no such
 * name. It also carried an `amountCents` the api layer stripped before sending: the
 * server recomputes the price from the spot and the minutes it actually authorised, so
 * the figure was never part of the request. It belongs to the form, which shows it.
 */
export type CreateBookingRequest = {
  spotId: string;
  /** Mirrors the spot's single-availability shape: `"YYYY-MM-DD"` -> slots. */
  booked: SingleAvailability;
  /**
   * The car that will park. Required, 1–16 characters — the same rule the user form
   * holds a plate to, because this is the booking form's copy of it.
   *
   * Sent rather than dereferenced from the renter server-side: booking-service has no
   * mirror of a user, and which car is coming is a choice made per booking anyway.
   */
  licensePlate: string;
};
