import { apiFetch } from "@/api/king";
import type {
  BookingListItem,
  Me,
  OwnerViewBooking,
  OwnerViewSpot,
  PayoutListItem,
  PublicViewSpot,
  SpotListItem,
} from "@/types/view";

// Every read the client makes, over plain REST.
//
// This replaced eleven GraphQL documents and the urql client under them. `apiFetch`
// already carries the three things every request needs — the bearer token, the
// 401-refresh-and-retry, and `X-Await-Version` — so there is nothing left for an
// exchange chain to do.
//
// ## One function, one endpoint, one shape
//
// No function here takes a flag that changes what comes back. `/spots` used to mean
// "mine" bare and "the map" with `?lng&lat&meters`; `/spots/:id` used to serve the
// public drawer, the edit form and the manage screen from one response that was cut
// per-caller server-side. Each of those is now its own path answering one shape, so the
// type on the left of a `useQuery` says exactly which columns were selected.
//
// ## The caller's id is never sent
//
// `SPOTS_OWNED`, `BOOKINGS_RENTED` and `PAYOUTS` each passed the reader's own id as a
// variable. That was safe only because a table permission clause independently refused
// everyone else's rows; with the clauses gone it would be a request rather than a
// claim, so the server takes the caller from the verified token and everything the
// caller owns lives under `/me`.

async function get<T>(path: string): Promise<T> {
  const res = await apiFetch(path);
  if (!res.ok) throw new Error(`${path} failed: ${res.status}`);
  return (await res.json()) as T;
}

/**
 * Query keys, in one place.
 *
 * vue-query caches by key, so a key is what a write has to name to invalidate a read.
 * Kept together because the two ends are otherwise a string literal in a component and
 * a matching one in a mutation handler, which is exactly the pair that drifts.
 *
 * Hierarchical on purpose: `["spots"]` invalidates every spot query including
 * `["spots", id]` and `["spots", id, "manage"]`, which is what a create, an edit or a
 * delete wants.
 */
export const viewKeys = {
  me: ["me"] as const,
  spots: ["spots"] as const,
  mySpots: ["spots", "mine"] as const,
  nearby: (lng: number, lat: number, meters: number) =>
    ["spots", "near", lng, lat, meters] as const,
  spot: (id: string) => ["spots", id] as const,
  spotManage: (id: string) => ["spots", id, "manage"] as const,
  bookings: ["bookings"] as const,
  myBookings: ["bookings", "mine"] as const,
  booking: (id: string) => ["bookings", id] as const,
  payouts: ["payouts"] as const,
};

/** The caller's own profile, chosen by the server from the verified claim. */
export const fetchMe = () => get<Me>("/api/view/me");

/** The caller's own listings, newest first. Excludes deleted ones, keeps inactive. */
export const fetchMySpots = () => get<SpotListItem[]>("/api/view/me/spots");

/** The caller's own bookings as a renter, newest first. */
export const fetchMyBookings = () =>
  get<BookingListItem[]>("/api/view/me/bookings");

/** The caller's own withdrawal history, newest first. */
export const fetchPayouts = () => get<PayoutListItem[]>("/api/view/me/payouts");

/**
 * Spots within `meters` of a point.
 *
 * **Always excludes the caller's own.** `SPOTS_NEARBY` asked for that explicitly and
 * `SPOTS_IN_RADIUS` did not, which meant the map offered a host their own driveway as
 * somewhere to park. It is now unconditional server-side, so there is no flag here.
 */
export const fetchSpotsNear = (lng: number, lat: number, meters: number) =>
  get<SpotListItem[]>(
    `/api/view/spots/nearby?lng=${lng}&lat=${lat}&meters=${Math.round(meters)}`,
  );

/**
 * One spot as a prospective renter sees it: the listing plus what is still booked on
 * it, with no renter names, amounts or hold expiries attached.
 *
 * 404s for an inactive spot, including for its own host — a host looking at their
 * listing wants {@link fetchSpotManage}.
 */
export const fetchSpot = (id: string) =>
  get<PublicViewSpot>(`/api/view/spots/${id}`);

/**
 * One spot as its host sees it, with every booking on it in full.
 *
 * Serves the manage screen and the edit form: they render different fields but may read
 * the same ones, and both need the bookings — the form to stop a host removing a slot
 * someone has taken, the screen to show who is coming.
 */
export const fetchSpotManage = (id: string) =>
  get<OwnerViewSpot>(`/api/view/spots/${id}/manage`);

/** One booking. 404s for a caller who is neither the renter nor the host. */
export const fetchBooking = (id: string) =>
  get<OwnerViewBooking>(`/api/view/bookings/${id}`);
