/**
 * Stand-in responses for vue-query's `placeholderData`, so a screen renders its real
 * template straight away and `data-loading` masks it into a skeleton (see `main.css`)
 * until the answer arrives.
 *
 * Typed against the response types, so a field the templates start rendering is a type
 * error here rather than a hole in the skeleton. Strings are realistic lengths because
 * each one becomes a bar that wide.
 *
 * **Never let a placeholder id reach the server.** Every id here is `PLACEHOLDER_ID`; a
 * child that fetches by the id it is given has to be skipped or disabled while
 * `isPlaceholderData` is true — see `SpotRating` on HomeView's nearby cards.
 */
import type { Address, Availability, Booked } from "@/types/domain/spot";
import type { BalanceResponse } from "@/types/responses/view/BalanceResponse";
import type { HostBookingsPageResponse } from "@/types/responses/view/HostBookingsPageResponse";
import type { HostSpotsPageResponse } from "@/types/responses/view/HostSpotResponse";
import type { NearbyResponse } from "@/types/responses/view/NearbyResponse";
import type { NotificationResponse } from "@/types/responses/view/NotificationResponse";
import type { PublicSpotResponse } from "@/types/responses/view/PublicSpotResponse";
import type { RenterBookingsPageResponse } from "@/types/responses/view/RenterBookingResponse";
import type { HostSummaryResponse } from "@/types/responses/view/SummaryResponse";
import type { WalletResponse } from "@/types/responses/view/WalletResponse";

export const PLACEHOLDER_ID = "00000000-0000-0000-0000-000000000000";

/**
 * Whether a row is one of these. Lists mask per row with it —
 * `:data-loading="isPlaceholder(row.id)"` — which covers both the first load and the
 * one row {@link withLoadingRow} appends while the next page is on its way.
 */
export const isPlaceholder = (id: string) => id.startsWith(PLACEHOLDER_ID.slice(0, -1));

/**
 * The rows, plus one placeholder at the end while `loading`. How an infinite list shows
 * its next page coming: the real row markup, masked, rather than a "Loading…" line.
 */
export const withLoadingRow = <T>(rows: T[], loading: boolean, placeholder: T[]): T[] =>
  loading ? [...rows, placeholder[0]] : rows;

/** A transparent pixel: an `<img>` keeps its box, and the skeleton's muted fill shows. */
const IMAGE = "data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";

/** `n` copies, for the few rows a list skeleton shows. Ids repeat — keys must not. */
const rows = <T>(n: number, row: (i: number) => T): T[] => Array.from({ length: n }, (_, i) => row(i));
const id = (i: number) => PLACEHOLDER_ID.slice(0, -1) + i;

/** Tomorrow, so date helpers have a real day to format and nothing reads as past. */
const tomorrow = (() => {
  const d = new Date(Date.now() + 86_400_000);
  return d.toISOString().slice(0, 10);
})();
const booked: Booked = { [tomorrow]: [{ start: "09:00", end: "12:00" }] };

const address: Address = {
  line1: "Kapelstraat 12",
  line2: null,
  city: "Hasselt",
  postalCode: "3500",
  region: null,
  country: "BE",
  formatted: "Kapelstraat 12, 3500 Hasselt",
};

const availability: Availability = {
  weekly: { monday: [], tuesday: [], wednesday: [], thursday: [], friday: [], saturday: [], sunday: [] },
  single: {},
};

/** Infinite queries cache `{ pages, pageParams }`, not one response. */
const infinite = <T, P>(page: T, pageParam: P) => ({ pages: [page], pageParams: [pageParam] });

export const placeholders = {
  hostSummary: {
    spots: 3,
    activeSpots: 2,
    bookedNow: 1,
    bookings: 12,
    earnedCents: 12_345,
    earnedThisMonthCents: 12_345,
    earnedLastMonthCents: 10_000,
  } satisfies HostSummaryResponse,

  hostSpots: infinite<HostSpotsPageResponse, number>(
    {
      spots: rows(3, (i) => ({
        id: id(i),
        title: "Driveway near the centre",
        description: null,
        pricePerHour: 250,
        images: [IMAGE],
        active: true,
        address,
        availability,
        timezone: "Europe/Brussels",
      })),
      nextOffset: null,
      total: 3,
    },
    0,
  ),

  renterBookings: infinite<RenterBookingsPageResponse, number>(
    {
      bookings: rows(3, (i) => ({
        id: id(i),
        spotId: PLACEHOLDER_ID,
        status: "confirmed",
        amount: 750,
        booked,
        licensePlate: "1-ABC-123",
        endsAt: `${tomorrow}T12:00:00Z`,
        cancelReason: null,
        spot: {
          id: PLACEHOLDER_ID,
          title: "Driveway near the centre",
          images: [IMAGE],
          address,
          timezone: "Europe/Brussels",
        },
      })),
      nextOffset: null,
      total: 3,
    },
    0,
  ),

  hostBookings: infinite<HostBookingsPageResponse, number>(
    {
      bookings: rows(3, (i) => ({
        id: id(i),
        booked,
        licensePlate: "1-ABC-123",
        status: "confirmed",
        endsAt: `${tomorrow}T12:00:00Z`,
        amount: 750,
        renter: { id: PLACEHOLDER_ID, firstName: "Firstname", lastName: "Lastname", profilePicture: null },
      })),
      nextOffset: null,
      total: 3,
    },
    0,
  ),

  wallet: infinite<WalletResponse, string | undefined>(
    {
      // Not the current month: the list keys sections by month, and this one is
      // appended below real months while the next one loads.
      month: "2000-01",
      inCents: 12_345,
      outCents: 2_345,
      nextMonth: null,
      transactions: rows(4, (i) => ({
        id: id(i),
        kind: "in",
        amountCents: 750,
        occurredAt: `${tomorrow}T12:00:00Z`,
        pending: false,
        title: "Driveway near the centre",
        booked,
        timezone: "Europe/Brussels",
      })),
    },
    undefined,
  ),

  balance: { availableCents: 12_345, pendingCents: 0 } satisfies BalanceResponse,

  nearby: rows<NearbyResponse>(2, (i) => ({
    id: id(i),
    title: "Driveway near the centre",
    pricePerHour: 250,
    images: [IMAGE],
    lng: 0,
    lat: 0,
    availability,
  })),

  spot: {
    id: PLACEHOLDER_ID,
    title: "Driveway near the centre",
    pricePerHour: 250,
    images: [IMAGE],
    address,
    availability,
    timezone: "Europe/Brussels",
    host: { id: PLACEHOLDER_ID, firstName: "Firstname", lastName: "Lastname", profilePicture: null },
  } satisfies PublicSpotResponse,

  notifications: rows<NotificationResponse>(3, (i) => ({
    kind: "rate_booking",
    bookingId: id(i),
    spotTitle: "Driveway near the centre",
    at: `${tomorrow}T12:00:00Z`,
    seen: true,
  })),
};
