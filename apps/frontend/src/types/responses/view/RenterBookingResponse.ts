import type { Address, Booked } from "@/types/domain/spot";

/**
 * The spot card on a renter's booking.
 *
 * **EXCEPTION:** everywhere else a booking never carries its spot. A renter's booking
 * does, because every row of their list draws one. The full spot, with its host, is still
 * `/renter/spots/{id}`.
 */
export type RenterBookingSpotResponse = {
  id: string;
  title: string;
  images: string[];
  address: Address;
  /** `booked` is wall-clock in this zone; without it "upcoming" is answered wrong. */
  timezone: string;
};

/** `GET /api/view/renter/bookings` and `/renter/bookings/{id}` — one of the caller's own. */
export type RenterBookingResponse = {
  id: string;
  /** Kept beside `spot`, which is null until the spot is projected. */
  spotId: string;
  status: string;
  /** EUR cents. Unscoped: this endpoint only ever returns your own. */
  amount: number;
  booked: Booked;
  /** The car you said you would bring. */
  licensePlate: string;
  endsAt: string;
  /** `'spot_unavailable'` means the host withdrew, not that you cancelled. */
  cancelReason: string | null;
  /** The exception. Null while the spot has not been projected here yet. */
  spot: RenterBookingSpotResponse | null;
};

/** `GET /api/view/renter/bookings?scope=&status=&limit=&offset=`. */
export type RenterBookingsPageResponse = {
  bookings: RenterBookingResponse[];
  /** Null at the end of the list. */
  nextOffset: number | null;
  total: number;
};
