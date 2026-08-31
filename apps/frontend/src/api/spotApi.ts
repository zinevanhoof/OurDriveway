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
 * Saves an edit. The edit form sends its whole state: `images` is the host's whole
 * list of media URLs, kept and newly uploaded alike, already in display order — so
 * the server never has to diff anything to tell "unchanged" from "removed".
 *
 * Every field is optional server-side, and an omitted one means "leave alone". The
 * live switch uses that: it is this call with a body of `{ active }` and nothing
 * else, which is what keeps a toggle from resubmitting availability — the field
 * the backend cancels bookings over.
 */
export async function updateSpot(spotId: string, request: object): Promise<Response> {
  return record(await apiFetch(`/api/spot/${spotId}`, json("PATCH", request)));
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
