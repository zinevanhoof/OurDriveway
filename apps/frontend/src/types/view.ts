// What view-service answers with.
//
// **One type per `*Response` struct in `shared/src/responses/view.rs`**, named the same.
// Projection names never cross the wire — a projection is a group of columns a statement
// selects, and it changes when a statement needs another column, which is not a reason for
// a client to change. These are the contract; the projections are not.
//
// They must stay in step by hand: there is no schema between them. That was true of the
// GraphQL documents too — `graphql-codegen` was never wired up, so every result was `any`
// and a renamed field was a runtime `undefined` rather than a type error. These are at
// least written down.
//
// Every field is camelCase because the Rust types carry
// `#[serde(rename_all = "camelCase")]`.
//
// ## `| null` never means "you may not see this"
//
// It means **there is nothing** — a join that found no row, a booking nobody cancelled,
// a renter with nothing coming up. The fields a caller may not see are simply absent from
// the type, because the route that serves them selects different columns.

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

// ─── account ────────────────────────────────────────────────────────────────

/** A person as anyone may see them. Never carries an email. */
export type UserPublicResponse = {
  id: string;
  firstName: string;
  lastName: string;
  profilePicture: string | null;
};

/**
 * The caller's own profile — the one place `email`, `licensePlates` and `country` appear.
 *
 * `email` is not nullable: every projected row is created by a registration, which carries
 * one, and the column is `NOT NULL`.
 */
export type AccountProfileResponse = {
  firstName: string;
  lastName: string;
  profilePicture: string | null;
  email: string;
  licensePlates: string[];
  /**
   * ISO 3166-1 alpha-2, or null until the profile sets it.
   *
   * Scoped like `email` rather than like `licensePlates`: only its owner sees it. It
   * exists because Stripe will not open a connected account without a country and fixes it
   * permanently at creation, so a host sets it once, before onboarding.
   */
  country: string | null;
};

/**
 * `GET /api/view/account`.
 *
 * `profile` is null only between signup and its projection. `id` comes from the verified
 * claim rather than a row, so it always resolves.
 */
export type AccountResponse = {
  id: string;
  profile: AccountProfileResponse | null;
};

// ─── public ─────────────────────────────────────────────────────────────────

/**
 * A booking on a spot's page, as anyone may see it: **the availability answer.**
 *
 * Which slots are taken, until when, and whether they still block. No renter, no amount —
 * not nulled, not selected.
 */
export type PublicBookingResponse = {
  id: string;
  booked: Booked;
  status: string;
  endsAt: string;
};

/** `GET /api/view/public/spots/{id}` — one active spot as a prospective renter sees it. */
export type PublicSpotResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  address: Address;
  availability: SpotAvailability;
  timezone: string;
  /** Null while the host has not been projected here yet — an absent join. */
  host: UserPublicResponse | null;
  bookings: PublicBookingResponse[];
};

/**
 * `GET /api/view/public/spots/nearby` — one map pin.
 *
 * No `address` and no `active`: a pin is placed by coordinates, and the route only returns
 * live listings in the first place.
 */
export type SpotPinResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  lng: number;
  lat: number;
  /** The weekday-and-time filter is a client-side fold over this. */
  availability: SpotAvailability;
};

// ─── host ───────────────────────────────────────────────────────────────────

/** A booking on the host's own spot. Carries the renter and the amount. */
export type HostBookingResponse = {
  id: string;
  booked: Booked;
  status: string;
  endsAt: string;
  /** EUR cents. */
  amount: number;
  /** Null while the renter has not been projected here yet. */
  renter: UserPublicResponse | null;
};

/** `GET /api/view/host/spots/{id}` — one spot as its host sees it. */
export type HostSpotResponse = {
  id: string;
  title: string;
  description: string | null;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  /** The live switch. An inactive spot resolves here and nowhere else. */
  active: boolean;
  address: Address;
  availability: SpotAvailability;
  timezone: string;
  bookings: HostBookingResponse[];
};

/** `GET /api/view/host/spots` — one row of the host's own list. */
export type HostSpotListItemResponse = {
  id: string;
  title: string;
  /** EUR cents. */
  pricePerHour: number;
  images: string[];
  /** False is a paused listing, which looks identical to a live one otherwise. */
  active: boolean;
  address: Address;
};

/**
 * `GET /api/view/host/balance`.
 *
 * Two figures, not four: `earnedCents` and `paidOutCents` are the arithmetic behind
 * `availableCents` and no screen renders them, so they stop at the server.
 */
export type BalanceResponse = {
  /** Withdrawable now: settled income minus what has already been taken out. */
  availableCents: number;
  /** Earned but not settled yet — what `availableCents` will grow by. */
  pendingCents: number;
};

// ─── renter ─────────────────────────────────────────────────────────────────

/** Just enough of a spot to render a booking card. */
export type SpotCardResponse = {
  id: string;
  title: string;
  images: string[];
  /** `booked` is wall-clock in this zone; without it "upcoming" is answered wrong. */
  timezone: string;
  address: Address;
};

/** `GET /api/view/renter/bookings` and `/renter/bookings/{id}` — one of the caller's own. */
export type RenterBookingResponse = {
  id: string;
  status: string;
  /** EUR cents. Unscoped: this endpoint only ever returns your own. */
  amount: number;
  booked: Booked;
  endsAt: string;
  /** `'spot_unavailable'` means the host withdrew, not that you cancelled. */
  cancelReason: string | null;
  /** Null while the spot has not been projected here yet. */
  spot: SpotCardResponse | null;
};

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

// ─── wallet ─────────────────────────────────────────────────────────────────

/**
 * One line of the wallet: a single movement of money involving the caller.
 *
 * `GET /api/view/me/payouts` and its `PayoutListItem` are gone — withdrawals are one of
 * the four kinds below, in the same list as everything else that moved.
 */
export type WalletTransactionResponse = {
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
 * `GET /api/view/account/wallet?month=YYYY-MM` — one month, which is also one page.
 *
 * `nextMonth` is the cursor: the next older month that holds anything, or null at the end
 * of the history. Asking for the month after it would be asking for nothing.
 */
export type WalletMonthResponse = {
  /** `"YYYY-MM"`. */
  month: string;
  /** Everything that came in, positive. */
  inCents: number;
  /** Everything that went out, **positive**, and not counting withdrawals. */
  outCents: number;
  nextMonth: string | null;
  transactions: WalletTransactionResponse[];
};
