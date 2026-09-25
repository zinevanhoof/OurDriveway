<script setup lang="ts">
// Everything the caller has open, newest first. Opening it marks all of it seen, which
// clears the bell's badge — but the rows that were new stay highlighted while the
// screen is up, so the user can still tell what just arrived.
import { ref, watch } from "vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { useRouter } from "vue-router";

import { fetchNotifications } from "@/api/viewApi";
import { dismissNotification, markNotificationsSeen } from "@/api/userApi";
import { viewKeys } from "@/api/keys";
import { placeholders } from "@/lib/placeholders";
import { Text } from "@/components/base/text";
import DetailLayout from "@/components/layout/DetailLayout.vue";
import NotificationRow from "@/components/notification/NotificationRow.vue";
import RateBookingDrawer from "@/components/booking/RateBookingDrawer.vue";
import type { NotificationResponse } from "@/types/responses/view/NotificationResponse";

const queryClient = useQueryClient();
const router = useRouter();
const { data, isPlaceholderData } = useQuery({
  queryKey: viewKeys.notifications,
  queryFn: fetchNotifications,
  placeholderData: placeholders.notifications,
});

const key = (n: NotificationResponse) => `${n.kind}:${n.bookingId}`;

// What was unseen when the screen opened, taken once from the first answer — usually
// the bell's cached one. The list itself stays live, so a rated booking drops out while
// the screen is still up.
const fresh = ref(new Set<string>());
const stopSnapshot = watch(
  data,
  async (list) => {
    // The skeleton's rows are not what the user saw: wait for the real answer.
    if (!list || isPlaceholderData.value) return;
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
</script>

<template>
  <DetailLayout @close="router.back()" title="Notifications">
    <template #main>
      <Text v-if="data && !data.length" class="text-muted-foreground">You're all caught up.</Text>
      <div class="space-y-3">
        <NotificationRow v-for="n in data" :key="key(n)" :notification="n" :fresh="fresh.has(key(n))"
          @click="select(n)" />
      </div>
    </template>
  </DetailLayout>

  <RateBookingDrawer v-model:open="rateOpen" :booking-id="rating?.bookingId ?? null"
    :spot-title="rating?.spotTitle ?? null" />
</template>
