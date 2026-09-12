import type { Address } from "@/types/domain/spot";

/** `GET /api/view/host/spots` — one row of the host's own list. */
export type HostSpotListItemResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  /** False is a paused listing, which looks identical to a live one otherwise. */
  active: boolean;
  address: Address;
};
