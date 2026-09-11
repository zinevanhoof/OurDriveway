import type { Booked } from "@/types/domain/spot";
import type { SpotCardResponse } from "./SpotCardResponse";

/** `GET /api/view/renter/bookings` and `/renter/bookings/{id}` — one of the caller's own. */
export type RenterBookingResponse = {
  id: string;
  status: string;
  /** EUR cents. Unscoped: this endpoint only ever returns your own. */
  amount: number;
  booked: Booked;
  endsAt: string;
  /** `'spot_unavailable'` means the host withdrew, not that you cancelled. */
  cancelReason: string | null;
  /** Null while the spot has not been projected here yet. */
  spot: SpotCardResponse | null;
};
