<script setup lang="ts">
// The spot detail sheet, shared by the map (where it leads to booking) and the
// renter's booking list (where it is read-only). It owns its own query rather than
// taking a spot object, so a caller only has to know an id — which is all either
// caller has when the user taps.
import { useQuery } from "@urql/vue";
import { computed } from "vue";
import { FULL_SPOT } from "@/api/graphql/spot";
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import { formatDay, formatSlots, sortedDays } from "@/lib/bookingDates";
import { formatCents } from "@/lib/money";
import { gqlRecordId } from "@/lib/utils";
import { CalendarDays, Star } from "@lucide/vue";
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

// Paused until something is selected, so nothing runs at render time and there is
// one query total rather than one per pin.
const { data } = useQuery({
  query: FULL_SPOT,
  variables: computed(() => ({ id: gqlRecordId(props.spotId) })),
  pause: computed(() => props.spotId === null),
});

const spot = computed(() => data.value?.spot);

// The spot's own zone, not the viewer's — same rule as the card. Falls back to the
// booking's copy so the list still labels "Today" correctly before FULL_SPOT lands.
const timezone = computed(() => spot.value?.timezone ?? props.booking?.spot?.timezone);
const days = computed(() => sortedDays(props.booking));
</script>

<template>
  <Drawer v-model:open="open">
    <DrawerContent @close-auto-focus.prevent
      class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
      <div class="m-4 space-y-4">
        <div class="flex h-40 gap-4 overflow-x-auto snap-x snap-mandatory no-scrollbar">
          <!-- `only:` = the sole image, so it fills the row instead of leaving a gap. -->
          <img v-for="key in spot?.images" :key="key" :src="key"
            class="snap-center shrink-0 h-full w-auto only:w-full object-cover rounded-md border-border" />
        </div>
        <div>
          <div class="flex items-end justify-between">
            <div class="text-lg font-bold">{{ spot?.title }}</div>
            <div class="flex items-baseline text-xl font-extrabold text-primary">{{
              formatCents(Number(spot?.price_per_hour))
            }}
              <div class="text-xs text-muted-foreground font-medium">/hr</div>
            </div>
          </div>
          <div class="flex max-w-3/4 gap-1 items-center text-xs text-muted-foreground font-medium">
            {{ spot?.address?.formatted }}
          </div>
        </div>
        <!-- The whole schedule, which is what the card's "+N more" points at.
             Rows are the unit here rather than a paragraph of dates: a booking can
             span several days with different hours on each, and a renter reads this
             to answer "when am I due there", one line at a time. -->
        <div v-if="days.length" class="rounded-lg border border-border overflow-hidden">
          <div
            class="flex items-center gap-1.5 bg-muted/50 px-3 py-2 text-xs font-semibold text-muted-foreground">
            <CalendarDays :size="14" />
            {{ days.length }} {{ days.length === 1 ? "day" : "days" }} booked
          </div>
          <div v-for="([date, slots], i) in days" :key="date"
            class="flex items-baseline justify-between gap-4 px-3 py-2 text-sm"
            :class="i > 0 && 'border-t border-border'">
            <span class="font-semibold">{{ formatDay(date, timezone) }}</span>
            <span class="text-muted-foreground font-medium tabular-nums">
              {{ formatSlots(slots) }}
            </span>
          </div>
          <div v-if="booking?.amount != null"
            class="flex items-baseline justify-between border-t border-border bg-muted/50 px-3 py-2">
            <span class="text-xs font-semibold text-muted-foreground">Total</span>
            <span class="font-extrabold">{{ formatCents(booking.amount) }}</span>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <Avatar size="lg">
            <AvatarImage v-if="spot?.owner?.profilePicture" :src="spot?.owner.profilePicture" />
            <AvatarFallback
              :name="{ firstName: spot?.owner?.firstName, lastName: spot?.owner?.lastName }" />
          </Avatar>
          <div>
            <div class="font-semibold">{{ spot?.owner?.firstName }} {{ spot?.owner?.lastName }}</div>
            <div class="flex items-center gap-1 text-xs text-muted-foreground font-medium">
              <Star :size="16" class="fill-star text-star" />
              4.9 · 128 trips
            </div>
          </div>
        </div>
        <div class="space-y-2">
          <Button v-if="bookable" class="w-full h-11 font-bold" @click="emit('book')">
            Check availability & book
          </Button>
          <!-- Kept even when there's nothing to book: this is the line a renter
               looking at a booking they already hold most needs to read. -->
          <div class="text-xs text-muted-foreground font-medium text-center">
            Free cancellation up to 1 hour before
          </div>
        </div>
      </div>
    </DrawerContent>
  </Drawer>
</template>
