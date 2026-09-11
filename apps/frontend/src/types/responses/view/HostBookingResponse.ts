import type { Booked } from "@/types/domain/spot";
import type { UserPublicResponse } from "./UserPublicResponse";

/** A booking on the host's own spot. Carries the renter and the amount. */
export type HostBookingResponse = {
  id: string;
  booked: Booked;
  status: string;
  endsAt: string;
  /** EUR cents. */
  amount: number;
  /** Null while the renter has not been projected here yet. */
  renter: UserPublicResponse | null;
};
