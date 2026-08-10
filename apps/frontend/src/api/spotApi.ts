import { apiFetch } from "./king";
import { recordSeq } from "@/lib/awaitSeq";
import { readErrorDetail } from "@/lib/serverErrors";

/**
 * Creates a listing.
 *
 * Returns the raw Response rather than throwing, because the create form maps a
 * 422's `errors` map back onto its own fields — see `showServerErrors` there.
 * Every write here answers 202 with `{ seq }`: the event is in the log, but the
 * projections that answer reads are still catching up, and recording the seq is
 * what makes the next query wait for this write.
 */
export async function createSpot(request: object): Promise<Response> {
  return record(await apiFetch("/api/spot", json("POST", request)));
}

/**
 * Saves an edit. Same shape as create: `images` is the host's whole list of media
 * keys, kept and newly uploaded alike, already in display order — so the server
 * never has to diff anything to tell "unchanged" from "removed".
 */
export async function updateSpot(spotId: string, request: object): Promise<Response> {
  return record(await apiFetch(`/api/spot/${spotId}`, json("PATCH", request)));
}

/**
 * The live switch. Off stops new reservations; bookings already taken stay valid,
 * which is the whole difference from `deleteSpot`.
 */
export async function setSpotActive(spotId: string, active: boolean): Promise<void> {
  const res = await apiFetch(`/api/spot/${spotId}/active`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ active }),
  });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  recordSeq((await res.json()).seq);
}

/**
 * Withdraws the listing for good, and with it every booking it still owes — the
 * server cancels those and they become refunds. Not reversible from the UI.
 */
export async function deleteSpot(spotId: string): Promise<void> {
  const res = await apiFetch(`/api/spot/${spotId}`, { method: "DELETE" });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  recordSeq((await res.json()).seq);
}

function json(method: string, body: object): RequestInit {
  return {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

async function record(response: Response): Promise<Response> {
  if (response.ok) {
    const body = await response
      .clone()
      .json()
      .catch(() => null);
    recordSeq(body?.seq);
  }
  return response;
}
