import type { Booked } from "@/types/domain/spot";

/**
 * `GET /api/view/public/spots/{id}/bookings` — one booking that takes slots on a spot:
 * **the availability answer.**
 *
 * Which slots are taken, until when, and whether they still block. No renter, no amount —
 * not nulled, not selected.
 */
export type PublicBookingResponse = {
  id: string;
  booked: Booked;
  status: string;
  endsAt: string;
};
