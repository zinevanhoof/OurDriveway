import type { ClassValue } from "clsx";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// `gqlRecordId` and `plainUuid` are gone, and their absence is the point.
//
// An id used to have two spellings that were not interchangeable, and reaching for the
// wrong one failed in two different ways:
//
//   gqlRecordId  `u'<uuid>'` — the SurrealQL literal a GraphQL record LOOKUP took.
//                SurrealDB *rendered* a record id as `spot:0199…` but only *accepted*
//                `u'0199…'` back, so a bare uuid returned null. Silently: no error, no
//                warning, just nothing.
//   plainUuid    the bare uuid — for REST paths and bodies, and for GraphQL `where`
//                filters on `TYPE uuid` columns. Wrapping one of these at least said
//                so, with a 400 or a type-conversion error.
//
// Three GraphQL documents took both, for the same spot, as two separate variables —
// `spot(id: $id)` beside `bookings(where: { spot_id: { eq: $spotUuid } })` — with a
// comment on each explaining that passing either to the other silently returned
// nothing.
//
// There is one spelling now. Ids are uuids in the database, in the JSON, and in the
// URL path, so there is nothing to convert and nothing to get wrong.
