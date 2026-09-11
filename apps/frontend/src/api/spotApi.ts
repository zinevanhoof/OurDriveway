import { useMutation, useQueryClient } from "@tanstack/vue-query";

import { del, get, patch, post, query } from "./client";
import { viewKeys } from "./keys";
import type { CreateSpotRequest } from "@/types/requests/spot/CreateSpotRequest";
import type { UpdateSpotRequest } from "@/types/requests/spot/UpdateSpotRequest";
import type { AddressSuggestResponse } from "@/types/responses/spot/AddressSuggestResponse";

/**
 * Creates a listing.
 *
 * `createSpot` and `updateSpot` used to take `request: object`, which accepted
 * anything at all — so `CreateSpotRequest` sat in `types/` imported by nothing
 * while the create form hand-built a literal. They are typed now.
 */
export const createSpot = (body: CreateSpotRequest) =>
  post<void>("/api/spot", body);

/**
 * Saves an edit. The edit form sends its whole state: `images` is the host's whole
 * list of media URLs, kept and newly uploaded alike, already in display order — so
 * the server never has to diff anything to tell "unchanged" from "removed".
 *
 * Every field is optional server-side, and an omitted one means "leave alone". The
 * live switch uses that: it is this call with a body of `{ active }` and nothing
 * else, which is what keeps a toggle from resubmitting availability — the field
 * the backend cancels bookings over.
 */
export const updateSpot = (spotId: string, body: UpdateSpotRequest) =>
  patch<void>(`/api/spot/${spotId}`, body);

/**
 * Withdraws the listing for good, and with it every booking it still owes — the
 * server cancels those and they become refunds. Not reversible from the UI.
 */
export const deleteSpot = (spotId: string) => del<void>(`/api/spot/${spotId}`);

/**
 * Type-ahead suggestions, proxied through spot-service to keep the LocationIQ key
 * server-side.
 *
 * Each item already carries every `Address` field, so a pick fills the form with no
 * follow-up request.
 *
 * This used to swallow every failure with `return []`, which made a broken
 * proxy indistinguishable from an address that does not exist. It throws like
 * everything else now; the two call sites are debounced typeaheads that catch and
 * fall back to showing nothing, which is the same behaviour written down where it
 * can be seen.
 */
export const suggestAddress = (q: string) =>
  get<AddressSuggestResponse[]>(`/api/spot/address/suggest${query({ q })}`);

// ─── hooks ──────────────────────────────────────────────────────────────────

export function useCreateSpot() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createSpot,
    // `["spots"]` is a prefix, so this covers the host list, the public detail and
    // every nearby query in one.
    onSuccess: () => queryClient.invalidateQueries({ queryKey: viewKeys.spots }),
  });
}

export function useUpdateSpot() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ spotId, body }: { spotId: string; body: UpdateSpotRequest }) =>
      updateSpot(spotId, body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: viewKeys.spots }),
  });
}

export function useDeleteSpot() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteSpot,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: viewKeys.spots });
      // Deleting a spot cancels the bookings on it, which become refunds — so the
      // renter's list and the wallet are both stale now, not just the spot.
      queryClient.invalidateQueries({ queryKey: viewKeys.bookings });
      queryClient.invalidateQueries({ queryKey: viewKeys.wallet });
      queryClient.invalidateQueries({ queryKey: viewKeys.balance });
    },
  });
}
