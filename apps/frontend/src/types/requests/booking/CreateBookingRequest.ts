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
};
