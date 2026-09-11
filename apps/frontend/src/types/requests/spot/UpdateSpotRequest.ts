import type { Availability } from "@/types/domain/spot";

/**
 * `PATCH /api/spot/{id}`. `shared::requests::spot::UpdateSpotRequest`.
 *
 * Every field is optional and an omitted one means "leave alone". The live switch uses
 * that: it is this request with a body of `{ active }` and nothing else, which is what
 * keeps a toggle from resubmitting availability — the field the backend cancels bookings
 * over.
 *
 * No `address`: a spot's location is fixed at creation.
 *
 * `images` is the host's whole list of media URLs, kept and newly uploaded alike, already
 * in display order — so the server never has to diff anything to tell "unchanged" from
 * "removed".
 */
export type UpdateSpotRequest = {
  title?: string;
  description?: string;
  pricePerHourCents?: number;
  availability?: Availability;
  images?: string[];
  active?: boolean;
};
