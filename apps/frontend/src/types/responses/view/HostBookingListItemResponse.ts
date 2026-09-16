import type { Booked } from "@/types/domain/spot";
import type { UserPublicResponse } from "./UserPublicResponse";

/**
 * `GET /api/view/host/spots/{id}/bookings` — one booking on the host's own spot.
 *
 * No `endsAt`: it decides which tab a booking falls in and in what order, both
 * server-side, and nothing here renders it.
 *
 * `booked` stays because the row's date line and its slot count are both folds over it —
 * and because it is what lets the detail drawer open with no second request.
 */
export type HostBookingListItemResponse = {
  id: string;
  booked: Booked;
  licensePlate: string;
  status: string;
  /** EUR cents. Non-null: the host is a party to every booking on their own listing. */
  amount: number;
  /** Null while the renter has not been projected here yet. */
  renter: UserPublicResponse | null;
};
