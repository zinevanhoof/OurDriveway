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
  nextSlot,
  sortedDays,
  startOfWeek,
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

// The soonest slot still ahead — not the booking's first one.
//
// The ended slot is built to end at exactly the current minute, so `end > time` is
// false however the clock happens to fall. The successor runs to 23:59, which only
// stops being in the future during the final minute of the day there.
const ended = { start: "00:00", end: timeThere.slice(0, 5) };
const later = { start: timeThere.slice(0, 5), end: "23:59" };
assert.deepEqual(nextSlot({ booked: { [dateThere]: [ended, later] } }, TZ), [dateThere, later]);

// Same, but the day is spent: the next day it holds wins outright.
const tomorrowThere = new Date(`${dateThere}T00:00:00Z`);
tomorrowThere.setUTCDate(tomorrowThere.getUTCDate() + 1);
const nextDay = tomorrowThere.toISOString().slice(0, 10);
const morning = { start: "09:00", end: "10:00" };
assert.deepEqual(
  nextSlot({ booked: { [dateThere]: [ended], [nextDay]: [morning] } }, TZ),
  [nextDay, morning],
);

// A slot in progress is the next one — the renter is due at it right now.
assert.deepEqual(nextSlot(spanning, TZ), [dateThere, spanning.booked[dateThere][0]]);
assert.equal(isActiveNow(spanning, TZ), true);

// Nothing ahead, and out-of-order day keys still resolve to the earliest future one.
assert.equal(nextSlot(booking("2020-01-01", "09:00", "10:00"), TZ), null);
assert.equal(nextSlot({ booked: {} }, TZ), null);
assert.deepEqual(
  nextSlot({ booked: { "2031-06-02": [morning], "2030-06-01": [morning] } }, TZ),
  ["2030-06-01", morning],
);

// startOfWeek is the viewer's Monday midnight, and always within the last week.
const week = new Date(startOfWeek());
assert.equal(Number.isNaN(week.getTime()), false);
assert.equal(week.getDay(), 1);
assert.equal(week.getHours(), 0);
assert.equal(week.getMinutes(), 0);
const sinceWeekStart = Date.now() - week.getTime();
assert.ok(sinceWeekStart >= 0 && sinceWeekStart < 7 * 24 * 60 * 60 * 1000);

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
