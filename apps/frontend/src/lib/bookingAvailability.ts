import { getLocalTimeZone } from "@internationalized/date";
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

// What's still bookable on a date: open windows minus slots already taken by
// confirmed bookings.
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

// Index `spot_busy` rows by date. The view projects one row per (spot, date)
// with no renter identity attached, so this is a reshape rather than a join —
// it used to flatten each booking's `booked` map, which required reading the
// `booking` table that only the renter and owner can see.
export function mergeBusy(
  rows: { date: string; slots?: TimeSlot[] }[],
): Record<string, TimeSlot[]> {
  const out: Record<string, TimeSlot[]> = {};
  for (const row of rows) (out[row.date] ??= []).push(...(row.slots ?? []));
  return out;
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
  return "ok";
}
