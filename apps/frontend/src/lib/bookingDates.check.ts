// Self-check for bookingDates.ts. No test runner in this project, and this needs
// none — plain asserts, run it directly:
//
//   node --experimental-strip-types src/lib/bookingDates.check.ts
//
// The zone is deliberately Pacific/Kiritimati (UTC+14). For most of the day it is
// already *tomorrow* there, so any helper that quietly fell back to the viewer's
// clock gets caught here instead of only for users abroad.

import assert from "node:assert/strict";
import {
  canCancel,
  formatDay,
  formatSlots,
  isActiveNow,
  isUpcoming,
  sortedDays,
  todayIn,
} from "./bookingDates.ts";

const TZ = "Pacific/Kiritimati";
const pad = (n: number) => String(n).padStart(2, "0");

// sv-SE renders as "YYYY-MM-DD HH:MM:SS", which is the format under test.
const [dateThere, timeThere] = new Date()
  .toLocaleString("sv-SE", { timeZone: TZ })
  .split(" ");
const hour = Number(timeThere.slice(0, 2));

const booking = (date: string, start: string, end: string, status = "confirmed") => ({
  status,
  booked: { [date]: [{ start, end }] },
});

// The spot's zone decides what "today" is, never the viewer's.
assert.equal(todayIn(TZ), dateThere);
assert.equal(formatDay(dateThere, TZ), "Today");
assert.notEqual(formatDay("2026-12-25", TZ), "Today");

assert.equal(isUpcoming(booking(dateThere, "09:00", "10:00"), TZ), true);
assert.equal(isUpcoming(booking("2020-01-01", "09:00", "10:00"), TZ), false);

// Active now = the spot's wall clock falls inside a slot on the spot's today.
const spanning = booking(
  dateThere,
  `${pad(Math.max(0, hour - 1))}:00`,
  `${pad(Math.min(23, hour + 1))}:00`,
);
assert.equal(isActiveNow(spanning, TZ), true);
assert.equal(isActiveNow(booking(dateThere, "09:00", "10:00"), null), false);

// Cancelling: confirmed only, well clear of the cutoff, and fails closed.
const faraway = booking("2030-06-01", "09:00", "10:00");
assert.equal(canCancel(faraway, TZ), true);
assert.equal(canCancel({ ...faraway, status: "reserved" }, TZ), false);
assert.equal(canCancel(faraway, "Not/AZone"), false);
assert.equal(canCancel(faraway, null), false);
assert.equal(canCancel(booking("2020-01-01", "09:00", "10:00"), TZ), false);

// Ordering is by date, then by start within a day — `booked` is an unordered map.
assert.deepEqual(
  sortedDays({ booked: { "2026-09-02": [], "2026-08-31": [] } }).map(([d]) => d),
  ["2026-08-31", "2026-09-02"],
);
assert.equal(
  formatSlots([
    { start: "11:00", end: "12:00" },
    { start: "09:00", end: "10:00" },
  ]),
  "09:00–10:00, 11:00–12:00",
);

console.log("bookingDates: all checks passed");
