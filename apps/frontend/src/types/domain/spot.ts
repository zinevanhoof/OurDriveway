// Value shapes that travel in both directions, mirroring
// `shared/src/general_models/spot.rs`. Not requests and not responses — a request
// embeds an `Address`, a response returns one, and they are the same struct on the
// server.

/**
 * `shared::general_models::spot::Address`.
 *
 * There used to be a second `Address` in `types/view.ts` with the same name and a
 * different shape — `string | null` here, `?: string` there — so which one a component
 * got depended on which file it imported from. This is the one, and it matches the wire:
 * the server's `Option<String>` serializes to `null`, never to an absent key.
 *
 * No `lat`/`lng`. Those exist only on an autocomplete suggestion, which is a different
 * type for that reason — see `types/responses/spot/AddressSuggestResponse.ts`. The
 * create form never sends coordinates: spot-service re-geocodes `formatted` on submit and
 * does not trust a client's point.
 */
export type Address = {
  line1: string;
  line2: string | null;
  city: string;
  postalCode: string;
  region: string | null;
  country: string;
  formatted: string;
};

/**
 * `shared::general_models::spot::Availability`.
 *
 * Both halves are required, as they are on the wire. `types/view.ts` used to type this
 * as `SpotAvailability` from `lib/bookingAvailability.ts`, whose two fields are optional
 * — a weaker claim than the server actually makes, and the only field in that file that
 * named a helper type instead of the response struct.
 *
 * Still assignable to `SpotAvailability`, so the pure fold helpers in
 * `lib/bookingAvailability.ts` keep taking it unchanged; they stay permissive because
 * they also run over a half-built form.
 */
export type Availability = {
  weekly: WeeklyAvailability;
  single: SingleAvailability;
};

export type WeeklyAvailability = {
  monday: TimeSlot[];
  tuesday: TimeSlot[];
  wednesday: TimeSlot[];
  thursday: TimeSlot[];
  friday: TimeSlot[];
  saturday: TimeSlot[];
  sunday: TimeSlot[];
};

/** `"YYYY-MM-DD"` -> slots. Dates, not weekdays: a one-off overrides the weekly grid. */
export type SingleAvailability = Record<string, TimeSlot[]>;

/**
 * `{ start: "08:00", end: "18:00" }` — bare wall-clock, in the spot's timezone.
 *
 * No `rename_all` on the Rust side either, so the keys are these.
 */
export type TimeSlot = {
  start: string;
  end: string;
};

/**
 * `shared::general_models::booking::Booked` — `"YYYY-MM-DD"` -> the slots taken on it.
 *
 * `#[serde(transparent)]` on the server, so the newtype is invisible on the wire and
 * this is the bare map.
 */
export type Booked = Record<string, TimeSlot[]>;
