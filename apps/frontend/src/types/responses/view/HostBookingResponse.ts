import type { Booked } from "@/types/domain/spot";
import type { UserPublicResponse } from "./UserPublicResponse";

/**
 * `GET /api/view/host/spots/{id}/bookings` — one booking on the host's own spot.
 *
 * `booked` is what the row's date line and slot count fold over, and what lets the detail
 * drawer open with no second request.
 */
export type HostBookingResponse = {
  id: string;
  booked: Booked;
  licensePlate: string;
  status: string;
  endsAt: string;
  /** EUR cents. Non-null: the host is a party to every booking on their own listing. */
  amount: number;
  /** Null while the renter has not been projected here yet. */
  renter: UserPublicResponse | null;
};
