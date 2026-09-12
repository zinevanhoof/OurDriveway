import type { SpotFilter } from "@/types/SpotFilter";
import type { TimeSlot } from "@/types/domain/spot";
import { covers, type SpotAvailability } from "./bookingAvailability";

// Every requested slot must be covered by some available slot. An empty request list
// means "any availability that day", so the spot just needs at least one slot.
const dayOk = (available: TimeSlot[], requested: TimeSlot[]) =>
  requested.length === 0
    ? available.length > 0
    : requested.every((r) => available.some((a) => covers(a, r)));

// Whether a spot's availability satisfies the map filter. A specific date uses its own
// slots, else falls back to that weekday's recurring hours. Empty filter → matches all.
export function spotMatches(
  availability: SpotAvailability,
  filter: SpotFilter,
): boolean {
  const weekly = availability?.weekly ?? {};
  const single = availability?.single ?? {};
  return Object.entries(filter.single).every(([date, req]) =>
    dayOk(single[date] ?? weekly[req.weekday] ?? [], req.slots),
  );
}
