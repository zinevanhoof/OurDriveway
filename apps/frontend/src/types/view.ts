// What view-service answers with.
//
// One type per projection in `shared/src/projections/`, and they must stay in step with
// it — there is no schema between them any more. That was true of the GraphQL documents
// too: `graphql-codegen` was never wired up, so every result was `any` and a renamed
// field was a runtime `undefined` rather than a type error. These are at least written
// down.
//
// Every field is camelCase because the Rust types carry
// `#[serde(rename_all = "camelCase")]`. The Rust side also carries `#[sqlx(rename)]`
// aliases on the nested types — those are SQL-side only and change nothing here.
//
// ## Public vs owner is a type, not a nullable field
//
// A `| null` below always means **"there is nothing"** — a join that found no row, a
// booking nobody rated. It never means "you may not see this". The fields a stranger
// used to receive as `null` are simply absent from the public types, because the
// endpoint that serves them selects different columns.

import type { SpotAvailability } from "@/lib/bookingAvailability";
import type { TimeSlot } from "@/types/domain/spot";

/** `"YYYY-MM-DD"` -> slots, in the spot's timezone. Bare wall-clock strings. */
export type Booked = Record<string, TimeSlot[]>;

export type Address = {
  line1: string;
  line2: string | null;
  city: string;
  postalCode: string;
  region: string | null;
  country: string;
  formatted: string;
};

/** A person as anyone may see them. Never carries an email. */
export type PublicViewUser = {
  id: string;
  firstName: string;
  lastName: string;
  profilePicture: string | null;
};

/**
 * The caller's own profile — the one place `email` and `licensePlates` appear.
 *
 * `email` is not nullable: every projected row is created by a registration, which
 * carries one, and the column is `NOT NULL`.
 */
export type OwnerViewUser = PublicViewUser & {
  email: string;
  licensePlates: string[];
  /**
   * ISO 3166-1 alpha-2, or null until the profile sets it.
   *
   * Scoped like `email` rather than like `licensePlates`: only its owner sees it.
   * It exists because Stripe will not open a connected account without a country and
   * fixes it permanently at creation, so a host sets it once, before onboarding.
   */
  country: string | null;
};

/** `GET /api/view/me`. `profile` is null only between signup and its projection. */
export type Me = { id: string; profile: OwnerViewUser | null };

/** The columns of one spot, shared by both spot shapes below. */
type SpotFields = {
  id: string;
  ownerId: string;
  /** Null while the owner has not been projected here yet — an absent join. */
  owner: PublicViewUser | null;
  title: string;
  description: string | null;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  lng: number;
  lat: number;
  active: boolean;
  address: Address;
  availability: SpotAvailability;
  timezone: string;
};

/**
 * A booking on a spot's page, as anyone may see it: **the availability answer.**
 *
 * Which slots are taken, until when, and whether they still block. No renter, no
 * amount, no hold expiry — not nulled, not selected.
 */
export type PublicViewBooking = {
  id: string;
  booked: Booked;
  status: string;
  endsAt: string;
};

/** A booking as a party to it sees it — the host on their spot, or the renter. */
export type OwnerViewBooking = PublicViewBooking & {
  spotId: string;
  renterId: string;
  ownerId: string;
  /** Null while the renter has not been projected here yet. */
  renter: PublicViewUser | null;
  /** EUR cents. */
  amount: number;
  holdUntil: string | null;
  releaseReason: string | null;
  /** `'spot_unavailable'` means the host withdrew, not that you cancelled. */
  cancelReason: string | null;
  rating: number | null;
  createdAt: string;
};

/** `GET /api/view/spots/:id` — one active spot as a prospective renter sees it. */
export type PublicViewSpot = SpotFields & { bookings: PublicViewBooking[] };

/** `GET /api/view/spots/:id/manage` — one spot as its host sees it. */
export type OwnerViewSpot = SpotFields & { bookings: OwnerViewBooking[] };

/** The list shape: map pins (`/spots/nearby`) and the host's own list (`/me/spots`). */
export type SpotListItem = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  active: boolean;
  lng: number;
  lat: number;
  address: Address;
  availability: SpotAvailability;
};

/** Just enough of a spot to render a booking card. */
export type SpotCard = {
  id: string;
  title: string;
  images: string[];
  /** `booked` is wall-clock in this zone; without it "upcoming" is answered wrong. */
  timezone: string;
  address: Address;
};

/** `GET /api/view/me/bookings` — the caller's own bookings as a renter. */
export type BookingListItem = {
  id: string;
  status: string;
  /** EUR cents. Unscoped: this endpoint only ever returns your own. */
  amount: number;
  booked: Booked;
  endsAt: string;
  cancelReason: string | null;
  spotId: string;
  /** Null while the spot has not been projected here yet. */
  spot: SpotCard | null;
};

/**
 * One line of the wallet: a single movement of money involving the caller.
 *
 * `GET /api/view/me/payouts` and its `PayoutListItem` are gone — withdrawals are one of
 * the four kinds below, in the same list as everything else that moved.
 */
export type WalletTransaction = {
  /** `<uuid>:<kind>`. One payment yields two rows — the charge and its refund. */
  id: string;
  /**
   * What it is, which decides the icon. The *direction* is the sign of `amountCents`,
   * because a refund is money back to a renter and money away from a host.
   */
  kind: "in" | "out" | "refund" | "payout";
  /** Signed EUR cents, from the caller's point of view: what their balance did. */
  amountCents: number;
  /** ISO 8601. What the month grouping and the ordering are on. */
  occurredAt: string;
  /** Host income that has not settled yet, so it is not withdrawable. */
  pending: boolean;
  /** The spot's title. Null on a payout, and on a spot not projected yet. */
  title: string | null;
  /** The booking's slots, in the spot's own wall clock. Null on a payout. */
  booked: Booked | null;
  /** The spot's IANA zone, which is what `booked` is written in. */
  timezone: string | null;
};

/**
 * `GET /api/view/me/wallet?month=YYYY-MM` — one month, which is also one page.
 *
 * `nextMonth` is the cursor: the next older month that holds anything, or null at the
 * end of the history. Asking for the month after it would be asking for nothing.
 */
export type WalletMonth = {
  /** `"YYYY-MM"`. */
  month: string;
  /** Everything that came in, positive. */
  inCents: number;
  /** Everything that went out, **positive**, and not counting withdrawals. */
  outCents: number;
  nextMonth: string | null;
  transactions: WalletTransaction[];
};

/** `GET /api/view/me/balance` — a host's money. */
export type Balance = {
  /** Withdrawable now: settled income minus what has already been taken out. */
  availableCents: number;
  /** Everything earned and settled, ever. */
  earnedCents: number;
  paidOutCents: number;
  /** Earned but not settled yet — what `availableCents` will grow by. */
  pendingCents: number;
};
