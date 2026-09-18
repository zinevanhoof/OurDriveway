import { get, query } from "@/api/client";
import type { AccountResponse } from "@/types/responses/view/AccountResponse";
import type { BalanceResponse } from "@/types/responses/view/BalanceResponse";
import type {
  BookingScope,
  BookingStatus,
  HostBookingsPageResponse,
} from "@/types/responses/view/HostBookingsPageResponse";
import type {
  HostSpotResponse,
  HostSpotsPageResponse,
} from "@/types/responses/view/HostSpotResponse";
import type { NearbyResponse } from "@/types/responses/view/NearbyResponse";
import type { NextBookingResponse } from "@/types/responses/view/NextBookingResponse";
import type { NotificationResponse } from "@/types/responses/view/NotificationResponse";
import type { PublicBookingResponse } from "@/types/responses/view/PublicBookingResponse";
import type { PublicSpotResponse } from "@/types/responses/view/PublicSpotResponse";
import type {
  RenterBookingResponse,
  RenterBookingsPageResponse,
} from "@/types/responses/view/RenterBookingResponse";
import type { RenterSpotResponse } from "@/types/responses/view/RenterSpotResponse";
import type {
  HostSpotSummaryResponse,
  HostSummaryResponse,
  SpotSummaryResponse,
  UserSummaryResponse,
} from "@/types/responses/view/SummaryResponse";
import type { Booked } from "@/types/domain/spot";
import type { WalletResponse } from "@/types/responses/view/WalletResponse";

/** One window of a paged list. Omit both for the first twenty; pass back `nextOffset`. */
export type Page = { limit?: number; offset?: number };

/** One window of a booking list: a {@link Page} plus the tab and the statuses. */
export type BookingsPage = Page & { scope?: BookingScope; status?: BookingStatus[] };

const bookingsQuery = (params: BookingsPage) =>
  query({ ...params, status: params.status?.join(",") });

// Every read the client makes, over plain REST.
//
// This replaced eleven GraphQL documents and the urql client under them. `get`
// already carries the bearer token, the 401-refresh-and-retry, `X-Await-Version`
// and error parsing, so there is nothing left for an exchange chain to do. The
// private `get<T>` that used to be at the top of this file is now
// `api/client.ts`, shared by every module rather than by this one.
//
// ## Four namespaces, one predicate each
//
// The path says who may read it:
//
//   /api/view/public/…    no relationship required
//   /api/view/host/…      host_id = caller
//   /api/view/renter/…    renter_id = caller
//   /api/view/account/…   id = caller
//
// That replaced a set of names that shared nothing. `/me/spots` and `/spots/:id/manage`
// were the same audience under two spellings; `/bookings/:id` served a renter and a host
// from one server-side disjunction; `/spots/:id` meant "public" only by convention.
//
// ## One function, one endpoint, one shape
//
// No function here takes a flag that changes what comes back, and the type on the left of
// a `useQuery` names the exact `*Response` struct the server sends — never a projection,
// which is a group of columns a statement happens to select.
//
// ## The caller's id is never sent
//
// The three GraphQL documents these replace each passed the reader's own id as a
// variable. That was safe only because a table permission clause independently refused
// everyone else's rows; with the clauses gone it would be a request rather than a claim,
// so the server takes the caller from the verified token.

// ─── account ────────────────────────────────────────────────────────────────

/** The caller's own profile, chosen by the server from the verified claim. */
export const fetchAccount = () => get<AccountResponse>("/api/view/account");

/** What the caller has open, newest first. The bell's badge counts `seen: false`. */
export const fetchNotifications = () =>
  get<NotificationResponse[]>("/api/view/account/notifications");

/**
 * One month of the caller's money: charges as a host, charges as a renter, refunds either
 * way, and withdrawals — in one list, newest first.
 *
 * Omit `month` for the current one. The response says which month it is and which one to
 * ask for next, so a client never has to guess a date or walk through empty months.
 *
 * This replaced `fetchPayouts`. Withdrawal history was its own endpoint off its own table;
 * it is one of the four kinds here now, because a list of what left your balance that
 * omits what entered it is not a wallet.
 */
export const fetchWallet = (month?: string) =>
  get<WalletResponse>(`/api/view/account/wallet${query({ month })}`);

// ─── host ───────────────────────────────────────────────────────────────────

/** One window of the caller's own listings, newest first. Excludes deleted, keeps inactive. */
export const fetchHostSpots = (page: Page = {}) =>
  get<HostSpotsPageResponse>(`/api/view/host/spots${query(page)}`);

/**
 * One spot as its host sees it. The spot only: who is coming is
 * {@link fetchHostSpotBookings}, and the slots still taken {@link fetchHostSpotBooked}.
 *
 * Serves the manage screen and the edit form: they render different fields but may read
 * the same ones. This was `/spots/:id/manage`, a path segment shared with nothing.
 */
export const fetchHostSpot = (id: string) =>
  get<HostSpotResponse>(`/api/view/host/spots/${id}`);

/**
 * Every slot a reserved or confirmed booking still holds on the host's spot, merged into
 * one map. The edit form's warning before a host removes hours someone has taken.
 */
export const fetchHostSpotBooked = (id: string) =>
  get<Booked>(`/api/view/host/spots/${id}/booked`);

/**
 * One window of one spot's bookings: the manage screen's preview and the paged screen.
 *
 * Separate from the spot because a listing is edited rarely and the bookings on it change
 * under it constantly, so they are worth separate cache entries.
 *
 * Everything is optional: `scope` defaults to upcoming, `status` to every status, `limit`
 * to 20 (at most 50), `offset` to 0. Pass back whatever `nextOffset` said — `null` there
 * is the end of the list.
 */
export const fetchHostSpotBookings = (spotId: string, params: BookingsPage = {}) =>
  get<HostBookingsPageResponse>(
    `/api/view/host/spots/${spotId}/bookings${bookingsQuery(params)}`,
  );

/**
 * What the caller has to withdraw, and what is still ripening.
 *
 * This was `GET /api/payment/earnings`. It moved here with every other read — but the
 * figure a withdrawal actually pays out is still computed by payment-service inside the
 * transaction that pays it, so this one being a moment behind the projector can never
 * overpay anyone.
 */
export const fetchBalance = () => get<BalanceResponse>("/api/view/host/balance");

/** The caller's totals as a host — spots, completed bookings, earned. The profile row. */
export const fetchHostSummary = () => get<HostSummaryResponse>("/api/view/host/summary");

/** How one of the caller's spots is doing — completed bookings, earned, rating. 404s for anyone else. */
export const fetchHostSpotSummary = (id: string) =>
  get<HostSpotSummaryResponse>(`/api/view/host/spots/${id}/summary`);

// ─── renter ─────────────────────────────────────────────────────────────────

/**
 * One window of the caller's own bookings, under one tab: `upcoming` soonest first,
 * `past` most recent first.
 *
 * **Each carries a card of its spot** — the one exception to a booking never carrying
 * its spot, because every row of the list draws one.
 */
export const fetchRenterBookings = (params: BookingsPage = {}) =>
  get<RenterBookingsPageResponse>(`/api/view/renter/bookings${bookingsQuery(params)}`);

/**
 * One spot the caller has booked, whole, with its host. Unlike {@link fetchSpot} it still
 * answers after the host pauses or deletes the listing; it 404s for a spot the caller
 * never booked.
 */
export const fetchRenterSpot = (id: string) =>
  get<RenterSpotResponse>(`/api/view/renter/spots/${id}`);

/**
 * The soonest confirmed booking that has not ended yet, or `null`.
 *
 * **One row where the home screen used to fetch the whole history.** It filtered to
 * `confirmed` in the browser and ranked what was left by comparing
 * `"YYYY-MM-DDTHH:MM"` wall-clock strings, which orders wrong across time zones. The
 * server sorts on `endsAt`, which is an instant.
 *
 * `null` is the answer for a renter with nothing coming, not an error.
 */
export const fetchNextBooking = () =>
  get<NextBookingResponse | null>("/api/view/renter/bookings/next");

/** One of the caller's own bookings. 404s for anyone else, the host included. */
export const fetchRenterBooking = (id: string) =>
  get<RenterBookingResponse>(`/api/view/renter/bookings/${id}`);

// ─── public ─────────────────────────────────────────────────────────────────

/**
 * Spots within `meters` of a point.
 *
 * **Always excludes the caller's own.** `SPOTS_NEARBY` asked for that explicitly and
 * `SPOTS_IN_RADIUS` did not, which meant the map offered a host their own driveway as
 * somewhere to park. It is now unconditional server-side, so there is no flag here.
 */
export const fetchSpotsNear = (lng: number, lat: number, meters: number) =>
  get<NearbyResponse[]>(
    `/api/view/public/spots/nearby${query({
      lng,
      lat,
      meters: Math.round(meters),
    })}`,
  );

/**
 * One spot as a prospective renter sees it. The listing only — what is booked on it is
 * {@link fetchSpotBookings}.
 *
 * 404s for an inactive spot, including for its own host — a host looking at their listing
 * wants {@link fetchHostSpot}, a renter looking at one they booked {@link fetchRenterSpot}.
 */
export const fetchSpot = (id: string) =>
  get<PublicSpotResponse>(`/api/view/public/spots/${id}`);

/**
 * The bookings still taking slots on an active spot, with no renter names or amounts
 * attached. What the booking form subtracts from the open hours.
 */
export const fetchSpotBookings = (id: string) =>
  get<PublicBookingResponse[]>(`/api/view/public/spots/${id}/bookings`);

/** A spot's rating, for anyone. `rating` is null until somebody rates it. */
export const fetchSpotSummary = (id: string) =>
  get<SpotSummaryResponse>(`/api/view/public/spots/${id}/summary`);

/** A person's reputation as a host: completed bookings and rating. Zeroes for a non-host. */
export const fetchUserSummary = (id: string) =>
  get<UserSummaryResponse>(`/api/view/public/users/${id}/summary`);
