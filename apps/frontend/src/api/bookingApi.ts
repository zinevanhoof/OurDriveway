import { apiFetch } from "./king";
import { recordSeq } from "@/lib/awaitSeq";
import { readErrorDetail } from "@/lib/serverErrors";
import type { BookingDraft } from "@/types/requests/BookingRequest";

export type CreatedBooking = {
  /** Booking record key, for opening checkout and for release. */
  id: string;
};

/**
 * Creates a booking, which holds the chosen slots while checkout runs.
 *
 * The draft's `amountCents` is deliberately not sent: the server recomputes it
 * from the spot's price and the minutes it actually authorised. It no longer
 * comes back either — the amount charged is whatever the Stripe session says, and
 * this reply carries only the id needed to open one.
 */
export async function createBooking(
  draft: BookingDraft,
): Promise<CreatedBooking> {
  const res = await apiFetch("/api/booking", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ spotId: draft.spotId, booked: draft.booked }),
  });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));

  const body = await res.json();
  recordSeq(body.seq);
  return { id: body.id };
}

// There is no `confirm`. A booking becomes confirmed when Stripe's webhook reaches
// payment-service, which publishes the event booking-service acts on — see
// paymentApi.createIntent. A client that could confirm its own booking would not have
// to pay for it, so the endpoint was removed rather than left behind a guard.

/**
 * Gives the slots back now rather than making the next renter wait out the hold.
 *
 * `keepalive` is for the renter who closes the tab mid-checkout — apiFetch spreads
 * its options into fetch, so the request survives the unload while still carrying
 * the Authorization header a `sendBeacon` couldn't. Best effort either way: the
 * server-side expiry sweeper is the actual guarantee, this just stops a popular
 * slot sitting idle for the full hold when someone walks away.
 */
export async function release(bookingId: string, keepalive = false): Promise<void> {
  await settle(`/api/booking/${bookingId}`, "DELETE", keepalive);
}

/**
 * Withdraws a booking that was already paid for.
 *
 * Not the same thing as `release`, which only ends an unpaid hold and answers 409
 * for anything confirmed. The server re-checks the one-hour cutoff against the
 * *spot's* timezone and is the authority on it — the UI hiding the button is a
 * courtesy, not the rule.
 */
export async function cancel(bookingId: string): Promise<void> {
  await settle(`/api/booking/${bookingId}/cancel`, "POST");
}

async function settle(path: string, method: string, keepalive = false): Promise<void> {
  const res = await apiFetch(path, { method, keepalive });
  if (!res.ok) throw new Error((await readErrorDetail(res)).join(" "));
  recordSeq((await res.json()).seq);
}
