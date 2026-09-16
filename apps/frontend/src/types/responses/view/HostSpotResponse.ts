import type { Address, Availability } from "@/types/domain/spot";

/**
 * `GET /api/view/host/spots` and `/host/spots/{id}` — a spot as its host sees it.
 *
 * No bookings: the rows are `/host/spots/{id}/bookings` and the taken slots
 * `/host/spots/{id}/booked`.
 */
export type HostSpotResponse = {
  id: string;
  title: string;
  description: string | null;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  /** The live switch. False is a paused listing, which looks identical otherwise. */
  active: boolean;
  address: Address;
  availability: Availability;
  timezone: string;
};

/** `GET /api/view/host/spots?limit=&offset=` — one window of the host's own listings. */
export type HostSpotsPageResponse = {
  spots: HostSpotResponse[];
  /** Null at the end of the list. */
  nextOffset: number | null;
  total: number;
};
