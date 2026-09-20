<script setup lang="ts">
// The spot detail sheet, shared by the map (where it leads to booking) and a renter's
// booking — from "My bookings" or the home screen's next one — where it is read-only and
// lists that booking's schedule. It owns its own query rather than taking a spot object,
// so a caller only has to know an id — which is all any caller has when the user taps.
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { fetchRenterSpot, fetchSpot, fetchUserSummary } from "@/api/viewApi";
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
import SpotRating from "./SpotRating.vue";
import { isPlaceholder, placeholders } from "@/lib/placeholders";

const props = withDefaults(defineProps<{
  spotId: string | null;
  /** Show the book button. Off for a booking the renter already holds. */
  bookable?: boolean;
  /**
   * Read the spot as one the caller booked (`/renter/spots/{id}`) rather than as a public
   * listing. On for a booking the renter holds: that read still answers after the host
   * pauses or deletes the listing, where the public one 404s.
   */
  renter?: boolean;
  /**
   * A booking on this spot, when the renter is looking at one they hold. Its whole
   * schedule is listed — the card only had room for the first day.
   */
  booking?: any;
  /**
   * Whether the sheet takes the screen while it is open.
   *
   * Modal is right for the two list callers, where nothing behind the sheet is worth
   * touching. The map is the exception: a modal layer sets `pointer-events: none` on
   * the whole body (reka-ui's DismissableLayer) and only restores it when the layer
   * unmounts — which vaul delays by the full 500ms close animation. So every dismiss
   * left the pins dead for half a second. Non-modal keeps the map live throughout, and
   * tapping a second pin swaps this sheet's contents instead of closing it first.
   */
  modal?: boolean;
}>(), { modal: true });

const open = defineModel<boolean>("open", { default: false });
const emit = defineEmits<{ book: [] }>();

// Disabled until something is selected, so nothing runs at render time and there is
// one query total rather than one per pin.
//
// The listing only. The public read shares the `spots/<id>` key with the map's and the
// home screen's own copy, which feeds the booking form, so vue-query serves one from
// the other. The renter's read is a different key, because it is a different route.
const { data, isError, error, refetch, isPlaceholderData } = useQuery({
  queryKey: computed(() =>
    props.renter ? viewKeys.renterSpot(props.spotId ?? "") : viewKeys.spot(props.spotId ?? ""),
  ),
  queryFn: () => (props.renter ? fetchRenterSpot(props.spotId!) : fetchSpot(props.spotId!)),
  // A placeholder booking row mounts this sheet too, with a placeholder spot id.
  enabled: computed(() => props.spotId !== null && !isPlaceholder(props.spotId)),
  placeholderData: placeholders.spot,
});

const spot = computed(() => data.value);

// The spot's own zone, not the viewer's — same rule as the card. The schedule waits for
// it rather than labelling "Today" in the viewer's zone first.
const timezone = computed(() => spot.value?.timezone);
const days = computed(() => sortedDays(props.booking));

// The host's reputation, under their name. Its own read, once the host has resolved.
const hostId = computed(() => spot.value?.host?.id ?? null);
const { data: hostSummary } = useQuery({
  queryKey: computed(() => viewKeys.userSummary(hostId.value ?? "")),
  queryFn: () => fetchUserSummary(hostId.value!),
  enabled: computed(() => hostId.value !== null && !isPlaceholder(hostId.value)),
});

// Each figure only when there is one: a new host shows their name and nothing else,
// rather than "0 bookings".
const hostRating = computed(() =>
  hostSummary.value?.rating != null ? hostSummary.value.rating.toFixed(1) : null,
);
const hostBookings = computed(() => {
  const n = hostSummary.value?.bookings ?? 0;
  return n > 0 ? `${n} ${n === 1 ? "booking" : "bookings"}` : null;
});
</script>

<template>
  <Drawer v-model:open="open" :modal="modal">
    <!-- Non-modal still dismisses on an outside pointer*down*, which fires before the
         marker's click — the sheet would close and reopen on every pin-to-pin tap. The
         map closes it on a canvas click instead, so a tap on empty map still works. -->
    <DrawerContent @close-auto-focus.prevent :overlay="modal"
      @pointer-down-outside="(e) => { if (!modal) e.preventDefault() }"
      class="data-[vaul-drawer-direction=bottom]:mb-15">
      <!-- A failed read used to render this sheet with every field blank, which looks
           like a spot with no title, no price and no address rather than a failure. -->
      <div v-if="isError" class="m-4 space-y-2 text-center">
        <Text size="sm">{{ (error as ApiError).detail.join(' ') }}</Text>
        <Button variant="outline" size="sm" @click="() => refetch()">Try again</Button>
      </div>
      <!-- The sheet caps at 80vh, so a long schedule scrolls inside it. -->
      <!-- `data-loading` sits on what the spot query fills — photos, title, price,
           address, host — not on the sheet: the schedule comes from the booking the
           caller passed, and the rest is fixed copy. -->
      <div v-else class="m-4 space-y-4 overflow-y-auto no-scrollbar">
        <!-- `touch-pan-x`: without it the browser claims a vertical swipe here for
             scrolling and cancels the pointer stream, so vaul never sees the drag and
             the sheet won't close when the gesture starts on a photo. Declaring the
             strip horizontal-only leaves the vertical axis to the drawer. -->
        <div class="flex h-40 gap-4 overflow-x-auto touch-pan-x snap-x snap-mandatory no-scrollbar"
          :data-loading="isPlaceholderData">
          <!-- `only:` = the sole image, so it fills the row instead of leaving a gap. -->
          <img v-for="key in spot?.images" :key="key" :src="key"
            class="snap-center shrink-0 h-full w-auto only:w-full object-cover rounded-md border-border" />
        </div>
        <div :data-loading="isPlaceholderData">
          <SectionHeader>
            <Title size="lg">{{ spot?.title }}</Title>
            <template #action>
              <Money :cents="spot?.pricePerHour ?? 0" suffix="/hr" size="xl" weight="extrabold" tone="primary" />
            </template>
          </SectionHeader>
          <Text class="flex max-w-3/4 gap-1 items-center">
            {{ spot?.address?.formatted }}
          </Text>
          <SpotRating v-if="spotId" :spot-id="spotId" />
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
          <!-- Which car they said they would bring. On the schedule rather than
               somewhere else on the sheet, because "when and in what" is one thought —
               and this is the half a renter is most likely to have forgotten. -->
          <div v-if="booking?.licensePlate"
            class="flex items-baseline justify-between border-t border-border px-3 py-2">
            <Text as="span" size="sm" weight="semibold">Car</Text>
            <Text as="span" size="sm" class="tabular-nums">{{ booking.licensePlate }}</Text>
          </div>
          <div v-if="booking?.amount != null"
            class="flex items-baseline justify-between border-t border-border bg-muted/50 px-3 py-2">
            <Text as="span" weight="semibold">Total</Text>
            <Money :cents="booking.amount" size="md" weight="extrabold" />
          </div>
        </div>

        <div class="flex items-center gap-2" :data-loading="isPlaceholderData">
          <Avatar size="lg">
            <AvatarImage v-if="spot?.host?.profilePicture" :src="spot?.host.profilePicture" />
            <AvatarFallback :name="{ firstName: spot?.host?.firstName ?? '', lastName: spot?.host?.lastName ?? '' }" />
          </Avatar>
          <div>
            <Title weight="semibold">{{ spot?.host?.firstName }} {{ spot?.host?.lastName }}</Title>
            <Text v-if="hostRating || hostBookings" class="flex items-center gap-1">
              <template v-if="hostRating">
                <Star :size="16" class="fill-star text-star" />
                <span>{{ hostRating }}</span>
              </template>
              <!-- Spans, not bare text: adjacent text nodes merge into one anonymous
                   flex item, and `gap` only spaces items. -->
              <span v-if="hostRating && hostBookings">·</span>
              <span v-if="hostBookings">{{ hostBookings }}</span>
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
