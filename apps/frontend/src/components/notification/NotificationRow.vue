<script setup lang="ts">
// One row in the notifications list. Both kinds share the row — an icon, a headline and a
// timestamp — and differ only in what they say, which is why this is one component with a
// branch rather than two rows that drift the first time the padding changes.
//
// The row is always tappable, so `as="button"` and `interactive` live here; what the tap
// *does* is the caller's business and falls through as `@click`.
import { CalendarCheck, Star } from "@lucide/vue";

import { Surface } from "@/components/base/surface";
import { Text } from "@/components/base/text";
import { isPlaceholder } from "@/lib/placeholders";
import type { NotificationResponse } from "@/types/responses/view/NotificationResponse";

const { notification, fresh } = defineProps<{
  notification: NotificationResponse;
  /** Unseen when the screen opened — highlighted for as long as the screen is up. */
  fresh?: boolean;
}>();

const when = () =>
  new Date(notification.at).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
</script>

<template>
  <Surface as="button" type="button" orientation="horizontal" :data-loading="isPlaceholder(notification.bookingId)"
    class="gap-3 text-left w-full" :class="fresh && 'bg-primary/10 border-primary'">
    <template v-if="notification.kind === 'rate_booking'">
      <Star :size="20" class="shrink-0 fill-star text-star" />
      <div>
        <Text weight="semibold">Rate your parking at {{ notification.spotTitle ?? "your booking" }}</Text>
        <Text size="sm" class="text-muted-foreground">Ended {{ when() }}</Text>
      </div>
    </template>
    <template v-else-if="notification.kind === 'spot_booked'">
      <CalendarCheck :size="20" class="shrink-0 text-primary" />
      <div>
        <Text weight="semibold">
          {{ notification.renterName ?? "Someone" }} booked {{ notification.spotTitle ?? "your spot" }}
        </Text>
        <Text size="sm" class="text-muted-foreground">{{ when() }}</Text>
      </div>
    </template>
  </Surface>
</template>
