/** `POST /api/booking/{id}/rating`. `shared::requests::booking::RateBookingRequest`. */
export type RateBookingRequest = {
  /** 1 to 5. */
  rating: number;
};
