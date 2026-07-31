import type { TimeSlot } from './requests/CreateSpotRequest'

// The map availability filter: specific dates only. Each date carries a
// precomputed `weekday` so the matcher can fall back to a spot's recurring
// hours without doing date→weekday math. Empty `{ single:{} }` means
// "no filter" (matches all spots).
export type SpotFilter = {
  single: Record<string, { weekday: string; slots: TimeSlot[] }>
}
