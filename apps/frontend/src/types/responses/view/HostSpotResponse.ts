import type { Address, Availability } from "@/types/domain/spot";
import type { HostBookingResponse } from "./HostBookingResponse";

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
  bookings: HostBookingResponse[];
};
