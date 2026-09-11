<script setup lang="ts">
// The spot detail sheet, shared by the map (where it leads to booking) and the
// renter's booking list (where it is read-only). It owns its own query rather than
// taking a spot object, so a caller only has to know an id — which is all either
// caller has when the user taps.
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { fetchSpot } from "@/api/viewApi";
import { viewKeys } from "@/api/keys";
import type { ApiError } from "@/api/client";
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import { formatDay, formatSlots, sortedDays } from "@/lib/bookingDates";
import { CalendarDays, Star } from "@lucide/vue";
import { Text, Title } from "@/components/base/text";
import { Money } from "@/components/base/money";
import { SectionHeader } from "@/components/base/section-header";
import Avatar from "../ui/avatar/Avatar.vue";
import AvatarImage from "../ui/avatar/AvatarImage.vue";
import AvatarFallback from "../ui/avatar/AvatarFallback.vue";
import Button from "../ui/button/Button.vue";

const props = defineProps<{
  spotId: string | null;
  /** Show the book button. Off for a booking the renter already holds. */
  bookable?: boolean;
  /**
   * A booking on this spot, when the renter is looking at one they hold. Its whole
   * schedule is listed — the card only had room for the first day.
   */
  booking?: any;
}>();

const open = defineModel<boolean>("open", { default: false });
const emit = defineEmits<{ book: [] }>();

// Disabled until something is selected, so nothing runs at render time and there is
// one query total rather than one per pin.
//
// The endpoint returns the bookings too; this sheet does not read them, and the picker
// next door does the subtracting. Both share the `spots/<id>` key, so vue-query serves
// the second from cache exactly as urql's document dedupe did — and the three variables
// that used to be needed (a record-id spelling, a plain uuid, and a `now`) are one path
// parameter.
const { data, isError, error, refetch } = useQuery({
  queryKey: computed(() => viewKeys.spot(props.spotId ?? "")),
  queryFn: () => fetchSpot(props.spotId!),
  enabled: computed(() => props.spotId !== null),
});

const spot = computed(() => data.value);

// The spot's own zone, not the viewer's — same rule as the card. Falls back to the
// booking's copy so the list still labels "Today" correctly before FULL_SPOT lands.
const timezone = computed(() => spot.value?.timezone ?? props.booking?.spot?.timezone);
const days = computed(() => sortedDays(props.booking));
</script>

<template>
  <Drawer v-model:open="open">
    <DrawerContent @close-auto-focus.prevent
      class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
      <!-- A failed read used to render this sheet with every field blank, which looks
           like a spot with no title, no price and no address rather than a failure. -->
      <div v-if="isError" class="m-4 space-y-2 text-center">
        <Text size="sm">{{ (error as ApiError).detail.join(' ') }}</Text>
        <Button variant="outline" size="sm" @click="() => refetch()">Try again</Button>
      </div>
      <div v-else class="m-4 space-y-4">
        <div class="flex h-40 gap-4 overflow-x-auto snap-x snap-mandatory no-scrollbar">
          <!-- `only:` = the sole image, so it fills the row instead of leaving a gap. -->
          <img v-for="key in spot?.images" :key="key" :src="key"
            class="snap-center shrink-0 h-full w-auto only:w-full object-cover rounded-md border-border" />
        </div>
        <div>
          <SectionHeader>
            <Title size="lg">{{ spot?.title }}</Title>
            <template #action>
              <Money :cents="spot?.pricePerHour ?? 0" suffix="/hr" size="xl" weight="extrabold" tone="primary" />
            </template>
          </SectionHeader>
          <Text class="flex max-w-3/4 gap-1 items-center">
            {{ spot?.address?.formatted }}
          </Text>
        </div>
        <!-- The whole schedule, which is what the card's "+N more" points at.
             Rows are the unit here rather than a paragraph of dates: a booking can
             span several days with different hours on each, and a renter reads this
             to answer "when am I due there", one line at a time. -->
        <div v-if="days.length" class="rounded-lg border border-border overflow-hidden">
          <Text weight="semibold" class="flex items-center gap-1.5 bg-muted/50 px-3 py-2">
            <CalendarDays :size="14" />
            {{ days.length }} {{ days.length === 1 ? "day" : "days" }} booked
          </Text>
          <div v-for="([date, slots], i) in days" :key="date"
            class="flex items-baseline justify-between gap-4 px-3 py-2 text-sm"
            :class="i > 0 && 'border-t border-border'">
            <Title as="span" size="sm" weight="semibold">{{ formatDay(date, timezone) }}</Title>
            <Text as="span" size="sm" class="tabular-nums">
              {{ formatSlots(slots) }}
            </Text>
          </div>
          <div v-if="booking?.amount != null"
            class="flex items-baseline justify-between border-t border-border bg-muted/50 px-3 py-2">
            <Text as="span" weight="semibold">Total</Text>
            <Money :cents="booking.amount" size="md" weight="extrabold" />
          </div>
        </div>

        <div class="flex items-center gap-2">
          <Avatar size="lg">
            <AvatarImage v-if="spot?.host?.profilePicture" :src="spot?.host.profilePicture" />
            <AvatarFallback
              :name="{ firstName: spot?.host?.firstName ?? '', lastName: spot?.host?.lastName ?? '' }" />
          </Avatar>
          <div>
            <Title weight="semibold">{{ spot?.host?.firstName }} {{ spot?.host?.lastName }}</Title>
            <Text class="flex items-center gap-1">
              <Star :size="16" class="fill-star text-star" />
              4.9 · 128 trips
            </Text>
          </div>
        </div>
        <div class="space-y-2">
          <Button v-if="bookable" class="w-full h-11 font-bold" @click="emit('book')">
            Check availability & book
          </Button>
          <!-- Kept even when there's nothing to book: this is the line a renter
               looking at a booking they already hold most needs to read. -->
          <Text class="text-center">
            Free cancellation up to 1 hour before
          </Text>
        </div>
      </div>
    </DrawerContent>
  </Drawer>
</template>
