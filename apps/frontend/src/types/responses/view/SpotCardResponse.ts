import type { Address } from "@/types/domain/spot";

/** Just enough of a spot to render a booking card. */
export type SpotCardResponse = {
  id: string;
  title: string;
  images: string[];
  /** `booked` is wall-clock in this zone; without it "upcoming" is answered wrong. */
  timezone: string;
  address: Address;
};
