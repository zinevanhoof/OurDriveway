<script setup lang="ts">
// Everything the caller has open, newest first. Opening it marks all of it seen, which
// clears the bell's badge — but the rows that were new stay highlighted while the
// screen is up, so the user can still tell what just arrived.
import { ref, watch } from "vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { CalendarCheck, Star } from "@lucide/vue";
import { useRouter } from "vue-router";

import { fetchNotifications } from "@/api/viewApi";
import { dismissNotification, markNotificationsSeen } from "@/api/userApi";
import { viewKeys } from "@/api/keys";
import { Text } from "@/components/base/text";
import FullScreenLayoutComponent from "@/components/FullScreenLayoutComponent.vue";
import RateBookingDrawer from "@/components/booking/RateBookingDrawer.vue";
import type { NotificationResponse } from "@/types/responses/view/NotificationResponse";

const queryClient = useQueryClient();
const router = useRouter();
const { data } = useQuery({ queryKey: viewKeys.notifications, queryFn: fetchNotifications });

const key = (n: NotificationResponse) => `${n.kind}:${n.bookingId}`;

// What was unseen when the screen opened, taken once from the first answer — usually
// the bell's cached one. The list itself stays live, so a rated booking drops out while
// the screen is still up.
const fresh = ref(new Set<string>());
const stopSnapshot = watch(
  data,
  async (list) => {
    if (!list) return;
    queueMicrotask(() => stopSnapshot());
    const unseen = list.filter((n) => !n.seen);
    fresh.value = new Set(unseen.map(key));
    if (!unseen.length) return;
    await markNotificationsSeen();
    void queryClient.invalidateQueries({ queryKey: viewKeys.notifications });
  },
  { immediate: true },
);

const rating = ref<NotificationResponse & { kind: "rate_booking" } | null>(null);
const rateOpen = ref(false);

function select(n: NotificationResponse) {
  switch (n.kind) {
    case "rate_booking":
      rating.value = n;
      rateOpen.value = true;
      break;
    case "spot_booked":
      // Nothing else finishes this one, so the tap does. Not awaited: the route change
      // should not wait on it, and the invalidation follows it whenever it lands.
      void dismissNotification(n.kind, n.bookingId).then(() =>
        queryClient.invalidateQueries({ queryKey: viewKeys.notifications }),
      );
      void router.push({ name: "spot-bookings", params: { id: n.spotId } });
      break;
  }
}

const when = (at: string) =>
  new Date(at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
</script>

<template>
  <FullScreenLayoutComponent @close="router.back()" title="Notifications">
    <template #main>
      <Text v-if="data && !data.length" class="text-muted-foreground">You're all caught up.</Text>
      <div class="space-y-3">
        <button v-for="n in data" :key="key(n)" type="button" @click="select(n)"
          class="flex w-full items-center gap-3 rounded-lg border border-border p-3 text-left"
          :class="fresh.has(key(n)) && 'bg-primary/10 border-primary'">
          <template v-if="n.kind === 'rate_booking'">
            <Star :size="20" class="shrink-0 fill-star text-star" />
            <div>
              <Text weight="semibold">Rate your parking at {{ n.spotTitle ?? "your booking" }}</Text>
              <Text size="sm" class="text-muted-foreground">Ended {{ when(n.at) }}</Text>
            </div>
          </template>
          <template v-else-if="n.kind === 'spot_booked'">
            <CalendarCheck :size="20" class="shrink-0 text-primary" />
            <div>
              <Text weight="semibold">
                {{ n.renterName ?? "Someone" }} booked {{ n.spotTitle ?? "your spot" }}
              </Text>
              <Text size="sm" class="text-muted-foreground">{{ when(n.at) }}</Text>
            </div>
          </template>
        </button>
      </div>
    </template>
  </FullScreenLayoutComponent>

  <RateBookingDrawer v-model:open="rateOpen" :booking-id="rating?.bookingId ?? null"
    :spot-title="rating?.spotTitle ?? null" />
</template>
