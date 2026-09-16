import type { Address, Availability } from "@/types/domain/spot";
import type { UserPublicResponse } from "./UserPublicResponse";

/**
 * `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it.
 *
 * No bookings: the taken slots are `/public/spots/{id}/bookings`, which the booking form
 * reads when it opens.
 */
export type PublicSpotResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  address: Address;
  availability: Availability;
  timezone: string;
  /** Null while the host has not been projected here yet — an absent join. */
  host: UserPublicResponse | null;
};
