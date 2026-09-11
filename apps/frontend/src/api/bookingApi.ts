import { useMutation, useQueryClient } from "@tanstack/vue-query";

import { del, post } from "./client";
import { viewKeys } from "./keys";
import type { CreateBookingRequest } from "@/types/requests/booking/CreateBookingRequest";
import type { CreateBookingResponse } from "@/types/responses/booking/CreateBookingResponse";

/**
 * Creates a booking, which holds the chosen slots while checkout runs.
 *
 * The amount is deliberately not sent: the server recomputes it from the spot's
 * price and the minutes it actually authorised. It does not come back either — the
 * amount charged is whatever the Stripe session says, and this reply carries only
 * the id needed to open one.
 */
export const createBooking = (body: CreateBookingRequest) =>
  post<CreateBookingResponse>("/api/booking", body);

// There is no `confirm`. A booking becomes confirmed when Stripe's webhook reaches
// payment-service, which publishes the event booking-service acts on — see
// paymentApi.createSession. A client that could confirm its own booking would not
// have to pay for it, so the endpoint was removed rather than left behind a guard.

/**
 * Gives the slots back now rather than making the next renter wait out the hold.
 *
 * `keepalive` is for the renter who closes the tab mid-checkout — it is the one
 * reason `del` takes a `RequestInit` at all, since the request has to survive the
 * unload while still carrying the Authorization header a `sendBeacon` couldn't.
 * Best effort either way: the server-side expiry sweeper is the actual guarantee,
 * this just stops a popular slot sitting idle for the full hold when someone walks
 * away.
 */
export const release = (bookingId: string, keepalive = false) =>
  del<void>(`/api/booking/${bookingId}`, { keepalive });

/**
 * Withdraws a booking that was already paid for.
 *
 * Not the same thing as `release`, which only ends an unpaid hold and answers 409
 * for anything confirmed. The server re-checks the one-hour cutoff against the
 * *spot's* timezone and is the authority on it — the UI hiding the button is a
 * courtesy, not the rule.
 */
export const cancel = (bookingId: string) =>
  post<void>(`/api/booking/${bookingId}/cancel`);

// ─── hooks ──────────────────────────────────────────────────────────────────

export function useCreateBooking() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createBooking,
    // The spot's own page shows what is taken on it, so a hold changes that too.
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: viewKeys.bookings });
      queryClient.invalidateQueries({ queryKey: viewKeys.spots });
    },
  });
}

export function useReleaseBooking() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      bookingId,
      keepalive,
    }: {
      bookingId: string;
      keepalive?: boolean;
    }) => release(bookingId, keepalive),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: viewKeys.bookings });
      queryClient.invalidateQueries({ queryKey: viewKeys.spots });
    },
  });
}

export function useCancelBooking() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: cancel,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: viewKeys.bookings });
      queryClient.invalidateQueries({ queryKey: viewKeys.spots });
      // A cancellation is a refund, which moves money for both sides.
      queryClient.invalidateQueries({ queryKey: viewKeys.wallet });
      queryClient.invalidateQueries({ queryKey: viewKeys.balance });
    },
  });
}
