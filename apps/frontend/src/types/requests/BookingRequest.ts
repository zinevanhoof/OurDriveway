import { SingleAvailability } from "../domain/spot";

// What the booking form emits, ready to POST to the booking service. `booked`
// mirrors the spot's single-availability shape (Record<"YYYY-MM-DD", TimeSlot[]>),

// so it slots straight into the booking's `booked` field.
export type BookingDraft = {
  spotId: string;
  booked: SingleAvailability;
  /** EUR cents, display only — the server recomputes it from the spot's price. */
  amountCents: number;
};
