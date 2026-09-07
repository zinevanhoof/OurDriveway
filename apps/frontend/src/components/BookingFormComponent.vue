<script setup lang="ts">
import { formatCents } from '@/lib/money';
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import FullScreenLayoutComponent from "./FullScreenLayoutComponent.vue";
import Calendar from "./ui/calendar/Calendar.vue";
import { ArrowRight, CalendarDays, Clock, Plus, X } from "@lucide/vue";
import Button from "./ui/button/Button.vue";
import Input from "./ui/input/Input.vue";
import Separator from "./ui/separator/Separator.vue";
import FilterChips from "./map/FilterChips.vue";
import { Surface } from "@/components/base/surface";
import { Text, Title } from "@/components/base/text";
import { IconBox } from "@/components/base/icon-box";
import { Money } from "@/components/base/money";
import { SectionHeader } from "@/components/base/section-header";
import { toast } from "vue-sonner";

import { DateFormatter, DateValue, getLocalTimeZone, today } from "@internationalized/date";
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";

import * as bookingApi from "@/api/bookingApi";
import * as paymentApi from "@/api/paymentApi";
import type { TimeSlot } from "@/types/domain/spot";
import {
    type SpotAvailability,
    remainingWindows,
    subtract,
    toMin,
} from "@/lib/bookingAvailability";

const props = defineProps<{
    /**
     * Structural rather than `SpotDetail`, so the form does not depend on the whole
     * read-model shape for four fields. `pricePerHour` is now a plain number — it was
     * `number | string` because auto GraphQL rendered an `int` column as a string often
     * enough that both had to be accepted, hence the `Number()` at every use site.
     */
    spot?: {
        id: string;
        title: string;
        pricePerHour: number;
        address?: { formatted?: string };
        availability?: SpotAvailability;
    };
    /**
     * Slots already taken, as `mergeBooked()` of the spot's still-to-come bookings.
     *
     * Its own prop rather than a field on `spot`: it comes from a sibling query, not
     * from the spot row — there is no denormalized copy on the spot any more.
     */
    booked?: Record<string, TimeSlot[]>;
}>();

const router = useRouter();

const open = defineModel<boolean>({ required: true });
const emit = defineEmits<{ booked: [bookingId: string] }>();

// No reshaping and no filtering: everything in `booked` is taken, and a lapsed hold
// stops blocking when the expiry sweeper releases it server-side rather than being
// filtered out here.
const occupied = computed(() => props.booked ?? {});

// ─── Calendar: only host-open dates (minus bookings) within 90 days ───
const minDate = today(getLocalTimeZone());
const maxDate = minDate.add({ days: 90 });
const isDateDisabled = (date: DateValue) =>
    remainingWindows(props.spot?.availability, occupied.value, date).length === 0;

// ─── Date selection (source of truth, independent of picked slots) ───
const selectedDates = ref<DateValue[]>([]);
const activeKey = ref("");
const pickedSlots = ref<Record<string, TimeSlot[]>>({});

const dfShort = new DateFormatter("en-US", { weekday: "short", month: "short", day: "numeric" });
const sortedDates = computed(() =>
    [...selectedDates.value].sort((a, b) => a.toString().localeCompare(b.toString())),
);
const activeDate = computed(() => selectedDates.value.find((d) => d.toString() === activeKey.value));
const dateChips = computed(() =>
    sortedDates.value.map((d) => ({ key: d.toString(), label: dfShort.format(d.toDate(getLocalTimeZone())) })),
);

// reka-ui multiple-mode hands back undefined once the last date is cleared;
// normalize to an array and make a freshly-added date the active one to edit.
function onDatesChange(v: DateValue | DateValue[] | undefined) {
    const next = Array.isArray(v) ? v : v ? [v] : [];
    if (next.length > selectedDates.value.length) activeKey.value = next[next.length - 1].toString();
    selectedDates.value = next;
}
// Drop picked slots whose date left the selection; keep active on a live key.
watch(selectedDates, () => {
    const keys = sortedDates.value.map((d) => d.toString());
    const valid = new Set(keys);
    for (const k of Object.keys(pickedSlots.value)) if (!valid.has(k)) delete pickedSlots.value[k];
    if (!valid.has(activeKey.value)) activeKey.value = keys[keys.length - 1] ?? "";
});

// ─── Per active date: open windows minus what's already picked ───
const activePicked = computed(() => pickedSlots.value[activeKey.value] ?? []);
const displayWindows = computed(() =>
    activeDate.value
        ? subtract(remainingWindows(props.spot?.availability, occupied.value, activeDate.value), activePicked.value)
        : [],
);

// One editable {start,end} draft per display window, seeded to the window bounds
// and preserved across recomputes by a stable key.
const drafts = ref<Record<string, TimeSlot>>({});
watch(
    displayWindows,
    (wins) => {
        const next: Record<string, TimeSlot> = {};
        for (const win of wins) {
            const k = `${activeKey.value}|${win.start}-${win.end}`;
            next[k] = drafts.value[k] ?? { start: win.start, end: win.end };
        }
        drafts.value = next;
    },
    { immediate: true },
);
const windowRows = computed(() =>
    displayWindows.value.map((win) => {
        const key = `${activeKey.value}|${win.start}-${win.end}`;
        return { win, key, draft: drafts.value[key]! }; // seeded synchronously by the immediate watch
    }),
);

// Valid = both filled, on the 30-min grid, end after start, inside the window.
function slotValid(win: TimeSlot, d?: TimeSlot) {
    if (!d?.start || !d?.end) return false;
    const s = toMin(d.start);
    const e = toMin(d.end);
    return s % 30 === 0 && e % 30 === 0 && e > s && s >= toMin(win.start) && e <= toMin(win.end);
}
function addSlot(win: TimeSlot, d: TimeSlot) {
    if (!activeKey.value || !slotValid(win, d)) return;
    (pickedSlots.value[activeKey.value] ??= []).push({ start: d.start, end: d.end });
}
function removeSlot(i: number) {
    activePicked.value.splice(i, 1);
}

// ─── Pricing + totals ───
// pricePerHour is EUR cents (integer) — see lib/money.ts. No `Number()`: it arrives as
// a number now rather than as auto GraphQL's stringified int.
const pricePerHourCents = computed(() => props.spot?.pricePerHour ?? 0);
// Rounded because half-hour slots on an odd cent price give a fractional cent.
const slotCents = (s: TimeSlot) => Math.round(slotHours(s) * pricePerHourCents.value);
const slotHours = (s: TimeSlot) => (toMin(s.end) - toMin(s.start)) / 60;
const totals = computed(() => {
    let dates = 0;
    let slots = 0;
    let hours = 0;
    for (const arr of Object.values(pickedSlots.value)) {
        if (arr.length) dates++;
        slots += arr.length;
        for (const s of arr) hours += slotHours(s);
    }
    return { dates, slots, hours, amountCents: Math.round(hours * pricePerHourCents.value) };
});

const description = computed(() => {
    const parts = [`${formatCents(pricePerHourCents.value)}/hr`];
    if (props.spot?.address?.formatted) parts.push(props.spot.address.formatted);
    return parts.join(" · ");
});

// ─── Hold the slots, then hand off to checkout ───
//
// This component stops at the reservation. Paying happens at `/checkout`, which is a real
// route with a real URL — it has to be, because a redirect payment method destroys the
// page and a drawer cannot survive that. Everything that used to live here (the Payment
// Element, a countdown, unload guards releasing the hold on the way to the bank) went with
// it.
const busy = ref(false);

async function submit() {
    if (!totals.value.slots || !props.spot || busy.value) return;
    const booked: Record<string, TimeSlot[]> = {};
    for (const [k, arr] of Object.entries(pickedSlots.value)) if (arr.length) booked[k] = arr;

    busy.value = true;
    try {
        const booking = await bookingApi.createBooking({
            spotId: props.spot.id,
            booked,
            amountCents: totals.value.amountCents,
        });

        // The session is created here rather than by the checkout screen, so that screen
        // needs nothing but a session id in its URL — no booking id, and therefore one
        // code path for entry, reload, return and retry.
        const session = await paymentApi.createSession(booking.id);

        emit("booked", booking.id);
        void router.push({ path: "/checkout", query: { session_id: session.sessionId } });
    } catch (e: any) {
        // Someone took the slots between rendering and submitting, the host isn't open
        // then, or the session couldn't be created. In the last case the hold exists with
        // no checkout attached — "Continue payment" on the bookings list is the way back.
        toast.error("Couldn't start checkout", { description: e.message });
    } finally {
        busy.value = false;
    }
}


// The drawer only hides on close (component stays mounted), so clear the picks when it
// closes — reopening starts fresh.
//
// It no longer releases anything. It used to, and that became a landmine the moment
// `submit()` started navigating: closing the drawer on the way to checkout would have
// released the booking it had just created. Giving up a hold is now an explicit button on
// the checkout screen, with the expiry sweeper as the fallback for a closed tab.
watch(open, (o) => {
    if (!o) {
        selectedDates.value = [];
        activeKey.value = "";
        pickedSlots.value = {};
        drafts.value = {};
    }
});
</script>

<template>
    <Drawer :open="open" :dismissible="false">
        <DrawerContent @close-auto-focus.prevent
            class="h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] [&>div:first-child]:hidden data-[vaul-drawer-direction=bottom]:mt-0 data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:max-h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:rounded-none z-50">
            <FullScreenLayoutComponent @close="open = false" :title="spot?.title ?? 'Book this spot'"
                :description="description">
                <template #main>
                    <!-- Only the picker lives here now. Paying is `/checkout`, a route of
                         its own, because a redirect payment method destroys this page. -->
                        <div class="space-y-2">
                            <Title weight="extrabold" class="text-[15px]">Pick your dates</Title>
                            <Calendar multiple :model-value="(selectedDates as any)" @update:model-value="onDatesChange"
                                :min-value="minDate" :max-value="maxDate" :is-date-disabled="isDateDisabled"
                                class="bg-card rounded-lg border border-border" initial-focus />
                        </div>

                        <Surface v-if="!selectedDates?.length" variant="dashed" size="lg"
                            class="justify-center items-center gap-3 text-center">
                            <IconBox size="lg">
                                <CalendarDays />
                            </IconBox>
                            <div class="w-3/4">Tap the highlighted dates the host has opened up, then choose your time
                                slots.</div>
                        </Surface>

                        <div v-else class="space-y-2">
                            <FilterChips :items="dateChips" :active="activeKey" @select="activeKey = $event"
                                @remove="(k) => selectedDates = selectedDates.filter(d => d.toString() !== k)" />
                            <div class="space-y-1">
                                <Title weight="extrabold" class="text-[15px]">Choose your time</Title>
                                <Text>Add one or more 30-min time slots
                                    within each of the host's open windows.</Text>
                            </div>

                            <Surface v-if="activeDate" class="gap-3">
                                <SectionHeader>
                                    <Title>{{ dfShort.format(activeDate.toDate(getLocalTimeZone())) }}
                                    </Title>
                                    <template #action>
                                        <Text weight="bold" tone="primary">
                                            {{ activePicked.length ? `${activePicked.length} slot${activePicked.length >
                                                1 ? 's'
                                                : ''}` : 'No time yet' }}
                                        </Text>
                                    </template>
                                </SectionHeader>

                                <!-- Already-picked slots for this date -->
                                <div v-if="activePicked.length" class="space-y-1.5">
                                    <Surface v-for="(s, i) in activePicked" :key="`${s.start}-${s.end}-${i}`"
                                        variant="accent" size="sm" orientation="horizontal"
                                        class="justify-between gap-2 text-sm font-semibold">
                                        <span>{{ s.start }} – {{ s.end }}</span>
                                        <Text as="span" weight="semibold" class="ml-auto">
                                            {{ slotHours(s) }} hr · {{ formatCents(slotCents(s)) }}
                                        </Text>
                                        <button type="button" @click="removeSlot(i)"
                                            class="cursor-pointer opacity-70 hover:opacity-100">
                                            <X :size="14" />
                                        </button>
                                    </Surface>
                                </div>

                                <Separator v-if="activePicked.length && windowRows.length" />

                                <!-- Open windows still available: add a slot inside each -->
                                <div v-if="windowRows.length" class="space-y-2">
                                    <div v-for="{ win, key, draft } in windowRows" :key="key" class="space-y-1.5">
                                        <Text weight="bold" class="flex items-center gap-1">
                                            <Clock class="size-4" />
                                            Host open · {{ win.start }} - {{ win.end }}
                                        </Text>
                                        <div class="flex items-center gap-1">
                                            <Input type="time" step="1800" :min="win.start" :max="win.end"
                                                v-model="draft.start" class="flex-1" />
                                            <ArrowRight class="text-muted-foreground size-4 shrink-0" />
                                            <Input type="time" step="1800" :min="win.start" :max="win.end"
                                                v-model="draft.end" class="flex-1" />
                                            <Button size="icon" :disabled="!slotValid(win, draft)"
                                                @click="addSlot(win, draft)">
                                                <Plus />
                                            </Button>
                                        </div>
                                    </div>
                                </div>
                                <Text v-else-if="!activePicked.length">
                                    No open time left on this date.
                                </Text>
                            </Surface>
                        </div>
                </template>

                <template #footer>
                    <div v-if="totals.slots" class="flex items-center justify-between pb-2.5 text-sm font-bold">
                        <Text as="span" size="sm" weight="bold">
                            {{ totals.dates }} date{{ totals.dates > 1 ? 's' : '' }} ·
                            {{ totals.slots }} slot{{ totals.slots > 1 ? 's' : '' }} ·
                            {{ totals.hours }} hr{{ totals.hours !== 1 ? 's' : '' }}
                        </Text>
                        <Money :cents="totals.amountCents" size="sm" />
                    </div>
                    <!-- Holds the slots and hands off to /checkout. The figure here is the
                         picker's estimate; the server reprices from the minutes it actually
                         authorises, and that is what the checkout screen shows. -->
                    <Button class="w-full h-11 font-bold" :disabled="!totals.slots || busy" @click="submit">
                        Continue to payment
                        <ArrowRight />
                    </Button>
                </template>
            </FullScreenLayoutComponent>
        </DrawerContent>
    </Drawer>
</template>
