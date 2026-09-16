import type { Address } from "@/types/domain/spot";
import type { UserPublicResponse } from "./UserPublicResponse";

/**
 * `GET /api/view/renter/spots/{id}` — a spot the caller has booked, whole, with its host.
 *
 * Still answers after the host pauses or deletes the listing, unlike the public read.
 */
export type RenterSpotResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  address: Address;
  /** The bookings' `booked` maps are wall-clock in this zone. */
  timezone: string;
  /** Null while the host has not been projected here yet. */
  host: UserPublicResponse | null;
};
