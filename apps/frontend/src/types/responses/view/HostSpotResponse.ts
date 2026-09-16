import type { Address, Availability, Booked } from "@/types/domain/spot";

/** `GET /api/view/host/spots/{id}` — one spot as its host sees it. */
export type HostSpotResponse = {
  id: string;
  title: string;
  description: string | null;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  /** The live switch. An inactive spot resolves here and nowhere else. */
  active: boolean;
  address: Address;
  availability: Availability;
  timezone: string;
  /**
   * Every slot a reserved or confirmed booking still holds, merged. What the edit form
   * checks before a host removes hours someone has taken. The rows themselves are
   * `GET /host/spots/{id}/bookings`.
   */
  booked: Booked;
};
