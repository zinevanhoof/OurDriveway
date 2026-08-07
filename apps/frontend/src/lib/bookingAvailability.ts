import { getLocalTimeZone, parseDate } from "@internationalized/date";
import type { TimeSlot } from "@/types/domain/spot";

// Structural date type — both reka-ui's and @internationalized's DateValue satisfy
// it, sidestepping their nominal (#private) mismatch when a Calendar matcher hands
// us a date.
type DateLike = { toString(): string; toDate(timeZone: string): Date };

// getDay() index (0 = Sunday) → availability weekday key. Same convention as
// MapSearchFilterComponent, so a date without specific slots falls back to the
// spot's recurring weekly hours.
const WEEKDAY_KEYS = [
  "sunday",
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
];

// Availability as returned by GraphQL (both are `object` scalars).
export type SpotAvailability = {
  weekly?: Record<string, TimeSlot[]>;
  single?: Record<string, TimeSlot[]>;
};

export const toMin = (hhmm: string) => {
  const [h, m] = hhmm.split(":").map(Number);
  return h * 60 + m;
};

export const toHHMM = (min: number) =>
  `${String(Math.floor(min / 60)).padStart(2, "0")}:${String(min % 60).padStart(2, "0")}`;

// Subtract busy intervals from open windows, returning the free remainder. Each
// open window is trimmed/split around every overlapping busy interval; fully
// covered windows drop out. All math in minutes so "HH:MM" strings stay simple.
export function subtract(open: TimeSlot[], busy: TimeSlot[]): TimeSlot[] {
  const busyMin = busy
    .map((b) => [toMin(b.start), toMin(b.end)] as const)
    .filter(([s, e]) => e > s);
  const result: TimeSlot[] = [];
  for (const o of open) {
    let segments: [number, number][] = [[toMin(o.start), toMin(o.end)]];
    for (const [bs, be] of busyMin) {
      const next: [number, number][] = [];
      for (const [ss, se] of segments) {
        if (be <= ss || bs >= se) {
          next.push([ss, se]); // no overlap
          continue;
        }
        if (bs > ss) next.push([ss, bs]); // left remainder
        if (be < se) next.push([be, se]); // right remainder
      }
      segments = next;
    }
    for (const [s, e] of segments)
      if (e > s) result.push({ start: toHHMM(s), end: toHHMM(e) });
  }
  return result;
}

// A date's open windows: its own single slots override that weekday's recurring
// hours (same precedence as spotFilter). Empty array = nothing open that day.
export function resolveOpenWindows(
  availability: SpotAvailability | undefined,
  date: DateLike,
): TimeSlot[] {
  const iso = date.toString();
  const weekday = WEEKDAY_KEYS[date.toDate(getLocalTimeZone()).getDay()];
  return availability?.single?.[iso] ?? availability?.weekly?.[weekday] ?? [];
}

// "HH:MM" strings compare chronologically, so a window fully covers a slot when it
// starts no later and ends no earlier.
export const covers = (window: TimeSlot, slot: TimeSlot) =>
  window.start <= slot.start && window.end >= slot.end;

/**
 * Booked slots that `availability` would no longer cover, on or after `from`.
 *
 * This is the edit screen's warning: the host is about to remove hours somebody
 * already paid for. It is a courtesy, not the rule — booking-service re-runs the
 * same question when the event lands and is the one that actually cancels. Past
 * dates are skipped for the same reason it skips them: a booking that already
 * happened can't be withdrawn.
 */
export function bookedOutside(
  availability: SpotAvailability | undefined,
  occupied: Record<string, TimeSlot[]>,
  from: string,
): { date: string; slot: TimeSlot }[] {
  return Object.entries(occupied)
    .filter(([date]) => date >= from)
    .flatMap(([date, slots]) => {
      const open = resolveOpenWindows(availability, parseDate(date));
      return slots
        .filter((slot) => !open.some((window) => covers(window, slot)))
        .map((slot) => ({ date, slot }));
    });
}

// What's still bookable on a date: open windows minus what's already taken.
//
// `occupied` is the spot's `booked` field verbatim — no reshaping and no
// filtering. Everything in it is taken, including a slot someone is paying for
// right now; a hold that lapses is removed server-side by the expiry sweeper, so
// there is no expiry for this side to reason about.
export function remainingWindows(
  availability: SpotAvailability | undefined,
  occupied: Record<string, TimeSlot[]>,
  date: DateLike,
): TimeSlot[] {
  return subtract(
    resolveOpenWindows(availability, date),
    occupied[date.toString()] ?? [],
  );
}

// ponytail: runnable self-check for the interval math — call demo() from a scratch
// script (`bunx tsx` / node) if you touch subtract().
export function demo() {
  const eq = (a: TimeSlot[], b: TimeSlot[]) => {
    if (JSON.stringify(a) !== JSON.stringify(b))
      throw new Error(
        `expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`,
      );
  };
  const open = [{ start: "09:00", end: "12:00" }];
  eq(subtract(open, []), open); // no busy → unchanged
  eq(subtract(open, [{ start: "09:00", end: "10:30" }]), [
    { start: "10:30", end: "12:00" },
  ]); // trim left
  eq(subtract(open, [{ start: "10:30", end: "12:00" }]), [
    { start: "09:00", end: "10:30" },
  ]); // trim right
  eq(subtract(open, [{ start: "10:00", end: "11:00" }]), [
    // split
    { start: "09:00", end: "10:00" },
    { start: "11:00", end: "12:00" },
  ]);
  eq(subtract(open, [{ start: "08:00", end: "13:00" }]), []); // fully covered
  eq(subtract(open, [{ start: "13:00", end: "14:00" }]), open); // disjoint
  eq(
    subtract(
      [
        { start: "09:00", end: "12:00" },
        { start: "14:00", end: "16:00" },
      ],
      [{ start: "09:00", end: "12:00" }],
    ),
    [{ start: "14:00", end: "16:00" }],
  ); // window fully taken drops, other stays

  // bookedOutside: which paid slots an edit would cancel. 2026-08-03 is a Monday.
  const monday = { weekly: { monday: [{ start: "08:00", end: "18:00" }] }, single: {} };
  const at = (slots: TimeSlot[]) => ({ "2026-08-03": slots });
  const outside = (a: SpotAvailability, slots: TimeSlot[]) =>
    bookedOutside(a, at(slots), "2026-08-01").length;

  if (outside(monday, [{ start: "09:00", end: "10:00" }]) !== 0)
    throw new Error("a slot inside the hours must survive");
  if (outside(monday, [{ start: "17:00", end: "19:00" }]) !== 1)
    throw new Error("a slot running past closing must be flagged");
  if (outside({ weekly: {}, single: {} }, [{ start: "09:00", end: "10:00" }]) !== 1)
    throw new Error("closing the day must flag everything on it");
  // An empty single entry closes that date even though Monday is open — same
  // precedence the server applies, and the easiest one to get backwards.
  if (outside({ ...monday, single: at([]) }, [{ start: "09:00", end: "10:00" }]) !== 1)
    throw new Error("an empty single entry must close the day");
  // Already happened: nothing to cancel, so it is never flagged.
  if (bookedOutside({ weekly: {}, single: {} }, at([{ start: "09:00", end: "10:00" }]), "2026-09-01").length)
    throw new Error("past dates must be skipped");

  return "ok";
}
