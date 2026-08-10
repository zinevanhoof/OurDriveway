<script setup lang="ts">
// One booking card. Upcoming and Past are the same card — Past just loses the
// footer and the tap target, which is why this is one component with a flag and
// not two near-identical blocks that drift the first time a class changes.
import { computed, ref } from "vue";
import { Clock, MapPin, Navigation, X } from "@lucide/vue";
import { toast } from "vue-sonner";
import * as bookingApi from "@/api/bookingApi";
import { native } from "@/api/http";
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import Button from "@/components/ui/button/Button.vue";
import Separator from "@/components/ui/separator/Separator.vue";
import SpotDetailDrawer from "@/components/spot/SpotDetailDrawer.vue";
import { canCancel, formatDay, formatSlots, isActiveNow, sortedDays } from "@/lib/bookingDates";
import { formatCents } from "@/lib/money";
import { recordId } from "@/lib/utils";
import { imageUrl } from "@/lib/media";

const props = defineProps<{ booking: any; past?: boolean }>();
const emit = defineEmits<{ changed: [] }>();

const detailOpen = ref(false);
const confirmOpen = ref(false);
const cancelling = ref(false);

const timezone = computed(() => props.booking?.spot?.timezone);
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
  const address = props.booking?.spot?.address?.formatted;
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

async function cancel() {
  cancelling.value = true;
  try {
    const id = recordId(props.booking.id)!;
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
  <div class="p-3 space-y-2 border border-border shadow-xs bg-card rounded-md">
    <div class="flex items-start gap-3" :class="!past && 'cursor-pointer'" @click="!past && (detailOpen = true)">
      <img v-if="booking?.spot?.images?.[0]" :src="imageUrl(booking.spot.images[0])"
        class="w-20 h-20 shrink-0 rounded-lg object-cover" />
      <div class="flex-1 space-y-1">
        <div class="font-semibold">{{ booking?.spot?.title }}</div>
        <div class="flex gap-1 items-center text-xs text-muted-foreground font-medium">
          <MapPin :size="16" class="shrink-0" />
          {{ booking?.spot?.address?.line1 }} · {{ booking?.spot?.address?.city }}
        </div>
        <!-- Dropped once it's history: which Tuesday it was is not what someone
             scanning past bookings is looking for. -->
        <div v-if="!past && days.length" class="flex gap-1 items-center text-xs text-muted-foreground font-medium">
          <Clock :size="16" class="shrink-0" />
          <span>
            {{ formatDay(days[0][0], timezone) }} · {{ formatSlots(days[0][1]) }}
            <!-- Tapping the card shows the rest; the count is the invitation. -->
            <span v-if="days.length > 1" class="text-foreground">+{{ days.length - 1 }} more</span>
          </span>
        </div>
        <div class="flex items-baseline gap-0.5 font-extrabold">
          {{ formatCents(booking?.amount ?? 0) }}
          <div class="text-xs text-muted-foreground font-medium">total</div>
        </div>
        <div v-if="withdrawn" class="text-xs text-destructive font-semibold">
          The host withdrew this spot. Your refund is on the way.
        </div>
      </div>
      <span class="shrink-0 rounded-full px-2 py-0.5 text-xs font-semibold capitalize"
        :class="active ? 'bg-primary text-primary-foreground' : STATUS_CLASS[statusLabel] ?? 'bg-muted text-muted-foreground'">
        {{ active ? "Active now" : statusLabel }}
      </span>
    </div>

    <template v-if="showFooter">
      <Separator />
      <div class="flex">
        <button class="flex justify-center flex-1 items-center gap-1 font-medium" @click="directions">
          <Navigation :size="16" />
          Directions
        </button>
        <template v-if="cancellable">
          <Separator orientation="vertical" />
          <button class="flex justify-center flex-1 items-center gap-1 font-medium text-destructive"
            @click="confirmOpen = true">
            <X :size="16" />
            Cancel
          </button>
        </template>
      </div>
    </template>

    <!-- The same sheet the map opens, minus anything that would book it again, plus
         the full schedule the card only had room to summarise. -->
    <SpotDetailDrawer v-model:open="detailOpen" :spot-id="booking?.spot?.id ?? null"
      :booking="booking" />

    <!-- Defaults left alone on purpose: drag-to-dismiss, the handle, backdrop tap
         and Esc are all vaul's, and a confirmation is the last place to break the
         gesture someone already expects. -->
    <Drawer v-model:open="confirmOpen">
      <DrawerContent @close-auto-focus.prevent
        class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
        <div class="m-4 space-y-4">
          <div>
            <div class="text-lg font-bold">Cancel this booking?</div>
            <div class="text-sm text-muted-foreground font-medium">
              {{ booking?.spot?.title }} —
              <span v-if="days.length">{{ formatDay(days[0][0], timezone) }}</span>.
              The slots go straight back on the market.
            </div>
          </div>
          <div class="space-y-2">
            <Button variant="destructive" class="w-full h-11 font-bold" :disabled="cancelling" @click="cancel">
              {{ cancelling ? "Cancelling…" : "Yes, cancel it" }}
            </Button>
            <Button variant="outline" class="w-full h-11 font-bold" @click="confirmOpen = false">
              Keep booking
            </Button>
          </div>
        </div>
      </DrawerContent>
    </Drawer>
  </div>
</template>
