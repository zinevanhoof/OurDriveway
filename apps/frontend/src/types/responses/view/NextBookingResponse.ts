import type { Booked } from "@/types/domain/spot";
import type { SpotCardResponse } from "./SpotCardResponse";

/**
 * `GET /api/view/renter/bookings/next` — the home screen's next-up card.
 *
 * The same rows as {@link RenterBookingResponse} behind a different response, and the
 * clearest case for why every route has one: the card renders a title, a zone and the
 * slots, so it is not sent a status, an end instant or a cancel reason.
 *
 * `null` when nothing is coming — an answer, not a 404.
 */
export type NextBookingResponse = {
  id: string;
  booked: Booked;
  /** EUR cents. Read by the detail sheet the card opens, not by the card. */
  amount: number;
  spot: SpotCardResponse | null;
};
