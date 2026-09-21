<script setup lang="ts">
// One booking card in "My bookings". Upcoming and Past are the same card — Past just
// loses the footer, which is why this is one component with a flag and not two
// near-identical blocks that drift the first time a class changes.
//
// The spot's title, photo, address and zone come from `booking.spot` — the one exception
// to a booking never carrying its spot. Tapping the card opens the full spot sheet.
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { Clock, CreditCard, MapPin, Navigation, X } from "@lucide/vue";
import { toast } from "vue-sonner";
import * as bookingApi from "@/api/bookingApi";
import * as paymentApi from "@/api/paymentApi";
import { native } from "@/api/http";
import { ConfirmDrawer } from "@/components/base/confirm-drawer";
import { Badge } from "@/components/ui/badge";
import Separator from "@/components/ui/separator/Separator.vue";
import { Surface } from "@/components/base/surface";
import { Text, Title } from "@/components/base/text";
import { Money } from "@/components/base/money";
import SpotDetailDrawer from "@/components/spot/SpotDetailDrawer.vue";
import { canCancel, formatDay, formatSlots, isActiveNow, sortedDays } from "@/lib/bookingDates";
import type { RenterBookingResponse } from "@/types/responses/view/RenterBookingResponse";

const props = defineProps<{ booking: RenterBookingResponse; past?: boolean }>();
const emit = defineEmits<{ changed: [] }>();

const router = useRouter();

/** Null only while the spot has not been projected yet. */
const spot = computed(() => props.booking.spot);

const detailOpen = ref(false);
const confirmOpen = ref(false);
const cancelling = ref(false);
const resuming = ref(false);

const timezone = computed(() => spot.value?.timezone);
const days = computed(() => sortedDays(props.booking));
const active = computed(() => isActiveNow(props.booking, timezone.value));
/** Reserved holds get the release flow; only a paid booking can be cancelled. */
const reserved = computed(() => props.booking?.status === "reserved");
const cancellable = computed(
  () => !props.past && (reserved.value || canCancel(props.booking, timezone.value)),
);
/** Nothing to navigate to or cancel once the host has withdrawn it. */
const showFooter = computed(() => !props.past && !withdrawn.value);

// A booking whose days have all passed reads as "completed", not "confirmed" —
// "confirmed" is a promise about something still ahead.
//
// ponytail: derived for display only. `completed` is in both status ASSERTs but no
// Rust code writes it, and being past is already a pure function of `booked`, so
// persisting it would mean a sweeper, an event and a projector arm to store a fact
// we can compute. Add that the day something *else* needs to query it — payouts to
// the host being the obvious one.
const statusLabel = computed(() => {
  if (withdrawn.value) return "cancelled by host";
  return props.past && props.booking?.status === "confirmed" ? "completed" : props.booking?.status;
});

/**
 * The host pulled this booking out from under the renter — by deleting the listing
 * or by removing the hours it sat in.
 *
 * Worth its own state rather than a plain "cancelled": the renter didn't do this,
 * they are owed their money back, and a card that just says "cancelled" reads like
 * they did it themselves.
 */
const withdrawn = computed(() => props.booking?.cancelReason === "spot_unavailable");

// One colour per status so the state is readable without parsing the word. No
// `badge` primitive exists in ui/, and a span with a class map is the whole need.
const STATUS_CLASS: Record<string, string> = {
  reserved: "bg-amber-100 text-amber-800",
  confirmed: "bg-emerald-100 text-emerald-800",
  completed: "bg-slate-100 text-slate-700",
  released: "bg-red-100 text-red-800",
  cancelled: "bg-red-100 text-red-800",
  "cancelled by host": "bg-red-100 text-red-800",
};

function directions() {
  const address = spot.value?.address?.formatted;
  if (!address) return;
  const q = encodeURIComponent(address);
  // `geo:` is the whole point on Android: it resolves to the system chooser across
  // every installed navigation app — Waze, Google Maps, Organic Maps — and the
  // user's "Always" choice sticks. A maps.google.com link would skip that and go
  // straight to Google Maps, which is exactly what a Waze user doesn't want.
  //
  // ponytail: iOS gets Apple Maps and no choice, because iOS has no `geo:` handler
  // and no system-wide navigation default. Offering a real pick there means an
  // action sheet over comgooglemaps:// and waze:// by hand — worth it only if iOS
  // ships. `src-tauri/gen/` is Android-only today, so this branch is dormant.
  const url = native
    ? /iPhone|iPad|iPod/.test(navigator.userAgent)
      ? `maps://?daddr=${q}`
      : `geo:0,0?q=${q}`
    : `https://www.google.com/maps/dir/?api=1&destination=${q}`;

  if (native) import("@tauri-apps/plugin-opener").then((o) => o.openUrl(url));
  else window.open(url, "_blank", "noopener");
}

/**
 * Reopens the checkout for a hold that was never paid.
 *
 * Creates the session rather than linking straight at a URL, because the session id is
 * the only thing `/checkout` takes and this row doesn't have one. That is safe to call
 * repeatedly: creation is idempotent per booking, so an abandoned checkout resumes on the
 * *same* Stripe session rather than opening a second payable one.
 */
async function resume() {
  resuming.value = true;
  try {
    const id = props.booking.id;
    const session = await paymentApi.createSession(id);
    void router.push({ path: "/checkout", query: { session_id: session.sessionId } });
  } catch (e: any) {
    toast.error("Couldn't reopen that checkout", { description: e.message });
  } finally {
    resuming.value = false;
  }
}

async function cancel() {
  cancelling.value = true;
  try {
    const id = props.booking.id;
    // Two different endpoints behind one button: releasing an unpaid hold and
    // cancelling a paid booking are separate transitions on the server, and it
    // answers 409 if you aim the wrong one at a booking.
    if (reserved.value) await bookingApi.release(id);
    else await bookingApi.cancel(id);
    confirmOpen.value = false;
    toast.success("Booking cancelled");
    emit("changed");
  } catch (e: any) {
    toast.error("Couldn't cancel that booking", { description: e.message });
  } finally {
    cancelling.value = false;
  }
}
</script>

<template>
  <Surface variant="elevated" class="gap-2">
    <Surface variant="none" size="none" orientation="horizontal" class="items-start gap-3 cursor-pointer"
      @click="detailOpen = true">
      <img v-if="spot?.images?.[0]" :src="spot.images[0]" class="w-20 h-20 shrink-0 rounded-lg object-cover" />
      <div class="flex-1 min-w-0 space-y-1">
        <Title weight="semibold">{{ spot?.title }}</Title>
        <Text class="flex gap-1 items-center">
          <MapPin :size="16" class="shrink-0" />
          {{ spot?.address?.line1 }} · {{ spot?.address?.city }}
        </Text>
        <Text v-if="days.length" class="flex gap-1 items-center">
          <Clock :size="16" class="shrink-0" />
          <span>
            {{ formatDay(days[0][0], timezone) }} · {{ formatSlots(days[0][1]) }}
            <!-- Tapping the card shows the rest; the count is the invitation. -->
            <span v-if="days.length > 1" class="text-foreground">+{{ days.length - 1 }} more</span>
          </span>
        </Text>
        <Money :cents="booking?.amount ?? 0" suffix="total" size="md" weight="extrabold" />
        <Text v-if="withdrawn" weight="semibold" tone="destructive">
          The host withdrew this spot. Your refund is on the way.
        </Text>
      </div>
      <Badge :class="['capitalize', active ? 'bg-primary text-primary-foreground' : STATUS_CLASS[statusLabel] ?? 'bg-muted text-muted-foreground']">
        {{ active ? "Active now" : statusLabel }}
      </Badge>
    </Surface>

    <template v-if="showFooter">
      <Separator />
      <div class="flex">
        <button class="flex justify-center flex-1 items-center gap-1 font-medium" @click="directions">
          <Navigation :size="16" />
          Directions
        </button>
        <!-- A hold is an unfinished checkout, not a booking. Now that paying has its own
             URL there is a way back into it, which is the difference between "I closed the
             tab" and "I lose these slots for fifteen minutes". -->
        <template v-if="reserved && !past">
          <Separator orientation="vertical" />
          <button class="flex justify-center flex-1 items-center gap-1 font-medium text-primary"
            :disabled="resuming" @click="resume">
            <CreditCard :size="16" />
            {{ resuming ? "Opening…" : "Continue payment" }}
          </button>
        </template>
        <template v-if="cancellable">
          <Separator orientation="vertical" />
          <button class="flex justify-center flex-1 items-center gap-1 font-medium text-destructive"
            @click="confirmOpen = true">
            <X :size="16" />
            {{ reserved ? "Give up" : "Cancel" }}
          </button>
        </template>
      </div>
    </template>

    <!-- The same sheet the map opens, minus anything that would book it again, plus the
         full schedule, plate and total the card only had room to summarise. -->
    <SpotDetailDrawer v-model:open="detailOpen" :spot-id="booking.spotId" :booking="booking" renter />

    <!-- Defaults left alone on purpose: drag-to-dismiss, the handle, backdrop tap
         and Esc are all vaul's, and a confirmation is the last place to break the
         gesture someone already expects. -->
    <!-- A hold and a paid booking are different things to give up, and the copy says so:
         nothing has been charged for a hold, so "cancel" would overstate what is
         happening. -->
    <ConfirmDrawer v-model:open="confirmOpen"
      :title="reserved ? 'Give up these times?' : 'Cancel this booking?'"
      :confirm-label="reserved ? 'Yes, give them up' : 'Yes, cancel it'" pending-label="Releasing…"
      :pending="cancelling" :cancel-label="reserved ? 'Keep them' : 'Keep booking'" @confirm="cancel">
      {{ spot?.title }} —
      <span v-if="days.length">{{ formatDay(days[0][0], timezone) }}</span>.
      The slots go straight back on the market.
      <template v-if="reserved">You haven't been charged.</template>
    </ConfirmDrawer>
  </Surface>
</template>
