import type { ClassValue } from "clsx";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// An id has exactly two spellings, and they are not interchangeable. Both take
// either input form — an id off a GraphQL result (`spot:<uuid>`) or a bare uuid
// from a REST response — and differ only in what they emit.
//
// These are named for the FORM they produce, not the place they came from,
// because the whole failure mode is reaching for the wrong one: an earlier
// version of this file called the first `recordId`, which sounded like the
// general-purpose choice and got used for REST bodies, where it 400s.

/// `u'<uuid>'` — the SurrealQL literal, for a GraphQL record LOOKUP only:
/// `spot(id:)`, `user(id:)`, `booking(id:)`.
///
/// SurrealDB's two spellings disagree in a way nothing warns you about: it
/// *renders* a record id as `spot:0199aabb-…` but only *accepts* `u'0199aabb-…'`
/// back. Measured — a bare uuid returns null, hyphenated or not, and declaring
/// the field's type does not make it coerce.
export const gqlRecordId = (id: string | null | undefined) => {
  const key = plainUuid(id);
  return key ? `u'${key}'` : id;
};

/// The bare uuid — for everything else. REST paths and bodies (the handlers take
/// `Uuid`), and GraphQL `where` FILTERS on `TYPE uuid` fields like `owner_id`.
///
/// Wrapping one of these is not a silent failure at least: a filter answers
/// `Error converting value … to type: uuid` and a REST call answers 400.
export const plainUuid = (id: string | null | undefined) =>
  id?.includes(":") ? id.split(":").slice(1).join(":") : id;
