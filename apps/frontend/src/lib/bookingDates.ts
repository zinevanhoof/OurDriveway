// A booking's `booked` map is `"YYYY-MM-DD"` -> slots, and both the dates and the
// `"HH:MM"` slot times are the *spot's* wall clock with no zone attached.
//
// So every question here — is it upcoming, is that day today, is it happening
// right now, is it too late to cancel — is asked against the spot's zone, not the
// viewer's. A driveway in Lisbon rolls over to tomorrow when Lisbon does.
//
// What is deliberately *not* zone-aware is the display: the dates and times are
// printed exactly as stored. Converting them to the viewer's zone would move a
// 09:00 booking to 10:00 on their screen while the host still expects them at
// 09:00. The comparisons shift; the labels never do.
//
// The technique throughout is to read the current wall clock in the spot's zone
// and compare `"YYYY-MM-DD"` / `"HH:MM"` strings against `booked`. Same answers as
// building instants, with no offset arithmetic to get wrong.

import { DateFormatter, now, parseDate } from "@internationalized/date";

/** One slot of a booking, as the API returns it. */
export type Slot = { start: string; end: string };

/** The fields of a booking these helpers read. */
export type Booking =
  | {
      status?: string | null;
      booked?: Record<string, Slot[]> | null;
    }
  | null
  | undefined;

/** Minutes before a booking starts that cancelling closes. Mirrors the server. */
const CUTOFF_MINUTES = 60;

const dayFormat = new DateFormatter(navigator.language, {
  weekday: "short",
  month: "short",
  day: "numeric",
});

/** `[date, slots]` pairs, earliest first. ISO dates sort chronologically. */
export function sortedDays(booking: Booking): [string, Slot[]][] {
  return Object.entries(booking?.booked ?? {}).sort(([a], [b]) => a.localeCompare(b));
}

/**
 * Today's date where the *spot* is, as `"YYYY-MM-DD"`.
 *
 * Falls back to the viewer's own today when the zone is missing or unparseable —
 * a spot whose projection hasn't landed yet shouldn't make its booking vanish
 * from both tabs. `canCancel` fails closed instead, because that one guards money.
 */
export function todayIn(timezone: string | null | undefined): string {
  return wallClock(timezone)?.[0] ?? localToday();
}

/** A booking with any day still to come, in the spot's zone. Else it is Past. */
export function isUpcoming(booking: Booking, timezone: string | null | undefined): boolean {
  const today = todayIn(timezone);
  return sortedDays(booking).some(([date]) => date >= today);
}

/** `"Today"` for the spot's today, else a short `Mon, Aug 3`. */
export function formatDay(date: string, timezone: string | null | undefined): string {
  if (date === todayIn(timezone)) return "Today";
  // parseDate wants a bare calendar date, which is exactly what a key is. Rendered
  // as a date only — attaching a time would imply a zone the string doesn't carry.
  return dayFormat.format(parseDate(date).toDate("UTC"));
}

/** `09:00–10:00`, joined when a day holds several slots. Printed verbatim. */
export function formatSlots(slots: Slot[]): string {
  return [...slots]
    .sort((a, b) => a.start.localeCompare(b.start))
    .map((s) => `${s.start}–${s.end}`)
    .join(", ");
}

/** Whether the renter is inside one of their slots right now, in the spot's zone. */
export function isActiveNow(booking: Booking, timezone: string | null | undefined): boolean {
  const there = wallClock(timezone);
  if (!there) return false;
  const [date, time] = there;
  return (booking?.booked?.[date] ?? []).some((s) => time >= s.start && time < s.end);
}

/**
 * Whether Cancel should be offered.
 *
 * The server enforces the same rule and wins any disagreement; this only avoids
 * drawing a button that would answer 409. Unknown zone -> false, matching the
 * server's fail-closed choice: we don't offer a cancel we can't show is in time.
 */
export function canCancel(booking: Booking, timezone: string | null | undefined): boolean {
  if (booking?.status !== "confirmed") return false;
  const there = wallClock(timezone);
  const start = firstSlot(booking);
  if (!there || !start) return false;
  return minutesBetween(there, start) >= CUTOFF_MINUTES;
}

/** The earliest `[date, time]` a booking occupies, or null if it holds nothing. */
function firstSlot(booking: Booking): [string, string] | null {
  const [day] = sortedDays(booking);
  if (!day) return null;
  const [date, slots] = day;
  const earliest = [...slots].sort((a, b) => a.start.localeCompare(b.start))[0];
  return earliest ? [date, earliest.start] : null;
}

/** Wall clock in `timezone` as `["YYYY-MM-DD", "HH:MM"]`, or null if unknown. */
function wallClock(timezone: string | null | undefined): [string, string] | null {
  if (!timezone) return null;
  try {
    const t = now(timezone);
    return [`${t.year}-${pad(t.month)}-${pad(t.day)}`, `${pad(t.hour)}:${pad(t.minute)}`];
  } catch {
    // An IANA name this browser's tz database doesn't know.
    return null;
  }
}

function localToday(): string {
  const d = new Date();
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

// ponytail: both wall clocks are treated as being on one offset, so a span
// crossing a DST change is up to an hour out. It only ever decides whether the
// Cancel button is drawn within an hour of the boundary — the server does the real
// zoned conversion and refuses regardless. Parse properly here if that changes.
function minutesBetween([fromDate, fromTime]: [string, string], [toDate, toTime]: [string, string]) {
  const minutes = (date: string, time: string) =>
    Date.UTC(+date.slice(0, 4), +date.slice(5, 7) - 1, +date.slice(8, 10)) / 60_000 +
    +time.slice(0, 2) * 60 +
    +time.slice(3, 5);
  return minutes(toDate, toTime) - minutes(fromDate, fromTime);
}

const pad = (n: number) => String(n).padStart(2, "0");
