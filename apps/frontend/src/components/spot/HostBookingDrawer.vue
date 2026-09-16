<script setup lang="ts">
// One booking on the host's own spot, in full: who is coming, in what, when, and for
// how much.
//
// **It owns no query.** Unlike `SpotDetailDrawer`, which is handed a spot id and fetches
// the spot, every field here is already in the row the list rendered — so opening this
// costs nothing and works while offline-ish, and the paged list's response carries
// `booked` precisely so that stays true.
import { computed } from "vue";
import { CalendarDays, Car } from "@lucide/vue";

import { Drawer, DrawerContent } from "@/components/ui/drawer";
import { Badge } from "@/components/ui/badge";
import Avatar from "@/components/ui/avatar/Avatar.vue";
import AvatarImage from "@/components/ui/avatar/AvatarImage.vue";
import AvatarFallback from "@/components/ui/avatar/AvatarFallback.vue";
import { Text, Title } from "@/components/base/text";
import { Money } from "@/components/base/money";
import { formatDay, formatSlots, sortedDays } from "@/lib/bookingDates";
import type { HostBookingResponse } from "@/types/responses/view/HostBookingResponse";

const props = defineProps<{
  booking: HostBookingResponse | null;
  /** The spot's zone. The slots are wall-clock in it, and are printed as stored. */
  timezone?: string;
}>();

const open = defineModel<boolean>("open", { default: false });

const days = computed(() => sortedDays(props.booking));
const slots = computed(() => days.value.reduce((n, [, s]) => n + s.length, 0));

/** What a status is worth saying, and how loudly. `reserved` is a hold mid-checkout. */
const status = computed(() => {
  switch (props.booking?.status) {
    case "confirmed":
      return { label: "Paid", variant: "default" } as const;
    case "reserved":
      return { label: "Awaiting payment", variant: "secondary" } as const;
    case "cancelled":
      return { label: "Cancelled", variant: "destructive" } as const;
    case "released":
      return { label: "Expired", variant: "outline" } as const;
    default:
      return { label: props.booking?.status ?? "", variant: "outline" } as const;
  }
});
</script>

<template>
  <Drawer v-model:open="open">
    <DrawerContent @close-auto-focus.prevent class="data-[vaul-drawer-direction=bottom]:mb-15">
      <div v-if="booking" class="m-4 space-y-4">
        <div class="flex items-center gap-2">
          <Avatar size="lg">
            <AvatarImage v-if="booking.renter?.profilePicture" :src="booking.renter.profilePicture" />
            <AvatarFallback :name="{
              firstName: booking.renter?.firstName ?? '',
              lastName: booking.renter?.lastName ?? '',
            }" />
          </Avatar>
          <div class="flex-1">
            <!-- Null only while the renter has not been projected here yet, which is
                 seconds at most — but it is a real state and the row says so. -->
            <Title size="lg">
              {{ booking.renter ? `${booking.renter.firstName} ${booking.renter.lastName}` : 'A renter' }}
            </Title>
            <Text class="flex items-center gap-1">
              <Car :size="14" />
              {{ booking.licensePlate }}
            </Text>
          </div>
          <Badge :variant="status.variant">{{ status.label }}</Badge>
        </div>

        <!-- Every day and every slot, not a summary: this is the screen a host checks
             to answer "is my driveway free on Thursday", one line at a time. -->
        <div class="rounded-lg border border-border overflow-hidden">
          <Text weight="semibold" class="flex items-center gap-1.5 bg-muted/50 px-3 py-2">
            <CalendarDays :size="14" />
            {{ days.length }} {{ days.length === 1 ? "day" : "days" }} ·
            {{ slots }} {{ slots === 1 ? "slot" : "slots" }}
          </Text>
          <div v-for="([date, daySlots], i) in days" :key="date"
            class="flex items-baseline justify-between gap-4 px-3 py-2 text-sm"
            :class="i > 0 && 'border-t border-border'">
            <Title as="span" size="sm" weight="semibold">{{ formatDay(date, timezone) }}</Title>
            <Text as="span" size="sm" class="tabular-nums">{{ formatSlots(daySlots) }}</Text>
          </div>
          <div class="flex items-baseline justify-between border-t border-border bg-muted/50 px-3 py-2">
            <Text as="span" weight="semibold">Total</Text>
            <!-- What the renter paid, which is what this listing earned. A cancelled
                 booking keeps its figure: it was refunded, not unmade. -->
            <Money :cents="booking.amount" size="md" weight="extrabold" />
          </div>
        </div>
      </div>
    </DrawerContent>
  </Drawer>
</template>
