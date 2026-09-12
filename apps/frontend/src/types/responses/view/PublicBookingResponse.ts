import type { Booked } from "@/types/domain/spot";

/**
 * A booking on a spot's page, as anyone may see it: **the availability answer.**
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
