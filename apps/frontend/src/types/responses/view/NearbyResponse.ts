import type { Availability } from "@/types/domain/spot";

/**
 * `GET /api/view/public/spots/nearby` — one map pin.
 *
 * No `address` and no `active`: a pin is placed by coordinates, and the route only returns
 * live listings in the first place.
 */
export type NearbyResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  lng: number;
  lat: number;
  /** The weekday-and-time filter is a client-side fold over this. */
  availability: Availability;
};
