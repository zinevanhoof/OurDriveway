import type { Address } from "@/types/domain/spot";

/**
 * `GET /api/spot/address/suggest?q=`.
 * `shared::responses::spot::AddressSuggestResponse`.
 *
 * An `Address` plus the point the provider resolved. The coordinates live here and *only*
 * here — that is why the domain `Address` no longer carries two optional lat/lng fields
 * that were meaningless everywhere except on a suggestion.
 *
 * Each item already carries every `Address` field, so picking one fills the form with no
 * follow-up request. The form does not send the point on: spot-service re-geocodes
 * `formatted` on submit and never trusts a client's coordinates.
 */
export type AddressSuggestResponse = Address & {
  /** Null for a hit the provider gave no parseable point for. */
  lat: number | null;
  lng: number | null;
};
