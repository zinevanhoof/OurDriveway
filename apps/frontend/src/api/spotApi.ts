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
export async function createSpot(formData: FormData): Promise<Response> {
  return record(await apiFetch("/api/spot", { method: "POST", body: formData }));
}

/**
 * Saves an edit. Same multipart shape as create: `data` holds the JSON, `images`
 * holds only the *newly* picked files — the ones the host kept are URLs inside the
 * JSON, so the server can tell "unchanged" from "removed" without diffing.
 */
export async function updateSpot(spotId: string, formData: FormData): Promise<Response> {
  return record(await apiFetch(`/api/spot/${spotId}`, { method: "PATCH", body: formData }));
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

/**
 * Builds the multipart body both create and edit send.
 *
 * The image list is mixed — URLs for photos already uploaded, `File`s for ones just
 * picked — and this is where it splits: URLs ride along inside the JSON so the
 * server knows which existing photos survived, files become parts. Create sends an
 * `images` array too; it is simply always empty, and the server ignores the field.
 */
export function spotFormData(data: object, images: (string | File)[]): FormData {
  const form = new FormData();
  form.append(
    "data",
    JSON.stringify({ ...data, images: images.filter((i) => typeof i === "string") }),
  );
  for (const image of images) {
    if (image instanceof File) form.append("images", image);
  }
  return form;
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
