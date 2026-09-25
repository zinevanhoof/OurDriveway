import type { HostBookingResponse } from "./HostBookingResponse";

/**
 * `GET /api/view/host/spots/{id}/bookings?scope=&status=&limit=&offset=` — one window of
 * them.
 *
 * Limit and offset rather than the wallet's cursor: the same route serves the manage
 * screen's two-row preview and the paged list. What an offset does not survive is the
 * list moving underneath it — a booking cancelled between two requests can make a row
 * repeat or be skipped across the boundary.
 *
 * `nextOffset` is what the client asks for next. It never computes one itself, for the
 * same reason it never computes the wallet's `nextMonth`.
 */
export type HostBookingsPageResponse = {
  bookings: HostBookingResponse[];
  /** Null at the end of the list. */
  nextOffset: number | null;
  /** Every booking in this scope and status, not just this window. */
  total: number;
};

/** The two tabs. `upcoming` is what the server serves when `scope` is absent. */
export type BookingScope = "upcoming" | "past";

/** What `status` may name. Absent on the request means all of them. */
export type BookingStatus = "reserved" | "confirmed" | "cancelled" | "released";
