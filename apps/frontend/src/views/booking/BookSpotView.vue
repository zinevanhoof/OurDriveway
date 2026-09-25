<script setup lang="ts">
import { formatCents } from '@/lib/money';
import DetailLayout from "@/components/layout/DetailLayout.vue";
import Calendar from "@/components/ui/calendar/Calendar.vue";
import { ArrowRight, CalendarDays, Car, Clock, Plus, X } from "@lucide/vue";
import Button from "@/components/ui/button/Button.vue";
import Input from "@/components/ui/input/Input.vue";
import Separator from "@/components/ui/separator/Separator.vue";
import { FilterChips } from "@/components/base/filter-chips";
import { Surface } from "@/components/base/surface";
import { Text, Title } from "@/components/base/text";
import { IconBox } from "@/components/base/icon-box";
import { Money } from "@/components/base/money";
import { SectionHeader } from "@/components/base/section-header";
import { toast } from "vue-sonner";

import {
    Combobox,
    ComboboxAnchor,
    ComboboxInput,
    ComboboxItem,
    ComboboxList,
    ComboboxViewport,
} from "@/components/ui/combobox";

import { DateFormatter, DateValue, getLocalTimeZone, today } from "@internationalized/date";
import { computed, onMounted, ref, watch } from "vue";
import { COVERED } from "@/router/transition";
import { useRouter } from "vue-router";
import { useQuery } from "@tanstack/vue-query";
import type { AcceptableValue } from "reka-ui";

import * as bookingApi from "@/api/bookingApi";
import * as paymentApi from "@/api/paymentApi";
import { fetchAccount, fetchSpot, fetchSpotBookings } from "@/api/viewApi";
import { useUpdateUser } from "@/api/userApi";
import { viewKeys } from "@/api/keys";
import type { TimeSlot } from "@/types/domain/spot";
import {
    mergeBooked,
    remainingWindows,
    subtract,
    toMin,
} from "@/lib/bookingAvailability";

const { id } = defineProps<{ id: string }>();

const router = useRouter();

// The listing. Shares `viewKeys.spot(id)` with the map's and the home screen's own read,
// which is what makes arriving here instant: the spot sheet you tapped "book" on has
// already filled this key, so the form renders from cache and revalidates behind it.
const { data: spot } = useQuery({
    queryKey: computed(() => viewKeys.spot(id)),
    queryFn: () => fetchSpot(id),
});

// The taken slots, read by this form and nothing else — the spot carries none.
//
// `staleTime: 0`, so every entry reads what is taken *now*: this is the one query someone
// books against. Freshness, not correctness — the server's availability check is the
// authority; this only stops the picker offering slots it then has to retract.
const { data: taken } = useQuery({
    queryKey: computed(() => viewKeys.spotBookings(id)),
    queryFn: () => fetchSpotBookings(id),
    staleTime: 0,
});

// No filtering: everything this returns is taken, and a lapsed hold stops blocking when
// the expiry sweeper releases it server-side rather than being filtered out here.
const occupied = computed(() => mergeBooked(taken.value));

// ─── Calendar: only host-open dates (minus bookings) within 90 days ───
const minDate = today(getLocalTimeZone());
const maxDate = minDate.add({ days: 90 });
const isDateDisabled = (date: DateValue) =>
    remainingWindows(spot.value?.availability, occupied.value, date).length === 0;

// **The calendar waits for the page to finish arriving.** It is a `CalendarCell` and a
// trigger per day, each with its own handful of computeds — around a hundred component
// instances — and mounting that on the frame the page starts sliding is what made getting
// here stutter, most visibly coming off the map. A placeholder holds its exact height and
// it mounts once the screen is still.
//
// It is not the availability maths: `remainingWindows` across a whole month grid measures
// 0.1ms. It is the instance count.
//
// The `taken` query lands inside the same window, which is the second half of the win —
// its re-render of every cell used to happen mid-animation too.
const calendarReady = ref(false);
onMounted(() => setTimeout(() => (calendarReady.value = true), COVERED * 1000));

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
        ? subtract(remainingWindows(spot.value?.availability, occupied.value, activeDate.value), activePicked.value)
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

// ─── Which car ───
//
// The plates come from the renter's own user record, the same `["account"]` query the
// edit screen reads, so opening this form after adding a plate there needs no refetch.
// One is required: a host has to know what to expect on their driveway, and the server
// refuses a booking without one.
const { data: account } = useQuery({ queryKey: viewKeys.account, queryFn: fetchAccount });
const plates = computed(() => account.value?.user?.licensePlates ?? []);

const plateTerm = ref("");
const plateOpen = ref(false);

const typedPlate = computed(() => plateTerm.value.trim());
// **The box is the answer.** Picking from the list fills the input, so there is no second
// piece of state that can disagree with what the renter can see — and editing a plate
// back to nothing disables the button without anything having to notice.
const plate = computed(() =>
    typedPlate.value.length >= 1 && typedPlate.value.length <= 16 ? typedPlate.value : "",
);
// Filtered here rather than by the combobox (`:ignore-filter`), because the list also
// holds an item that is not a plate yet — the one that creates one.
const plateMatches = computed(() =>
    plates.value.filter((p) => p.toLowerCase().includes(typedPlate.value.toLowerCase())),
);
const sameAs = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
// Offer to create only what could be saved: the server holds a plate to 1–16 characters,
// and a plate they already own is a pick, not a create.
const newPlate = computed(
    () =>
        typedPlate.value.length > 0 &&
        typedPlate.value.length <= 16 &&
        !plates.value.some((p) => sameAs(p, typedPlate.value)),
);

function pickPlate(value: AcceptableValue) {
    plateTerm.value = String(value);
    plateOpen.value = false;
}

const { mutateAsync: saveUser } = useUpdateUser();

// ─── Pricing + totals ───
// pricePerHour is EUR cents (integer) — see lib/money.ts. No `Number()`: it arrives as
// a number now rather than as auto GraphQL's stringified int.
const pricePerHourCents = computed(() => spot.value?.pricePerHour ?? 0);
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

// ─── Hold the slots, then hand off to checkout ───
//
// This component stops at the reservation. Paying happens at `/checkout`, which is a real
// route with a real URL — it has to be, because a redirect payment method destroys the
// page and a drawer cannot survive that. Everything that used to live here (the Payment
// Element, a countdown, unload guards releasing the hold on the way to the bank) went with
// it.
const busy = ref(false);

async function submit() {
    if (!totals.value.slots || !plate.value || !spot.value || busy.value) return;
    const booked: Record<string, TimeSlot[]> = {};
    for (const [k, arr] of Object.entries(pickedSlots.value)) if (arr.length) booked[k] = arr;

    busy.value = true;
    try {
        // A plate typed here rather than picked is theirs from now on, so it goes on the
        // user record first — before the booking, so a save that fails cannot leave a
        // booking whose plate was never kept. The booking carries its own copy either
        // way: this list changes, and the car that took a slot does not.
        if (newPlate.value) {
            await saveUser({ licensePlates: [...plates.value, plate.value] });
        }

        // `amountCents` is not sent. The server recomputes the price from the spot
        // and the minutes it authorises; the figure on screen is display only.
        const booking = await bookingApi.createBooking({
            spotId: id,
            booked,
            licensePlate: plate.value,
        });

        // The session is created here rather than by the checkout screen, so that screen
        // needs nothing but a session id in its URL — no booking id, and therefore one
        // code path for entry, reload, return and retry.
        const session = await paymentApi.createSession(booking.id);

        // `replace`, not `push`: this screen has done its job, and leaving it on the stack
        // means Back from checkout lands on a picker whose slots are already held.
        void router.replace({ path: "/checkout", query: { session_id: session.sessionId } });
    } catch (e: any) {
        // Someone took the slots between rendering and submitting, the host isn't open
        // then, or the session couldn't be created. In the last case the hold exists with
        // no checkout attached — "Continue payment" on the bookings list is the way back.
        toast.error("Couldn't start checkout", { description: e.message });
    } finally {
        busy.value = false;
    }
}

// No reset on the way out. This was a drawer that stayed mounted and had to clear its own
// picks on close; as a route it is destroyed on leave and mounts fresh, so the state is
// gone with it.
//
// It releases nothing either. It used to, and that became a landmine the moment `submit()`
// started navigating: leaving on the way to checkout would have released the booking it
// had just created. Giving up a hold is an explicit button on the checkout screen, with
// the expiry sweeper as the fallback for a closed tab.
</script>

<template>
    <DetailLayout @close="router.back()" title="Book this spot" :show-action="totals.slots > 0">
        <template #main>
            <!-- The listing being booked. It used to be the header's title and a
                         one-line "€x/hr · address" underneath; the header is a centred
                         label now, and this is the size the thing deserves anyway — the
                         renter arrives here from a pin and is still deciding.

                         `touch-pan-x`: this screen renders inside a vaul drawer, and
                         without it the browser claims a vertical swipe on a photo for
                         scrolling and cancels the pointer stream, so the sheet won't
                         close when the gesture starts here. Same note as SpotDetailDrawer. -->
            <div class="flex h-40 gap-2 overflow-x-auto touch-pan-x snap-x snap-mandatory no-scrollbar">
                <!-- `only:` = the sole image, so it fills the row instead of
                             leaving a gap.

                             `decoding="async"`: a host's photo decodes at whatever size it
                             was uploaded, and the default is to do that on the main thread
                             — on the frame this page is sliding in on. -->
                <img v-for="key in spot?.images" :key="key" :src="key" alt="" decoding="async"
                    class="snap-center shrink-0 h-full w-auto only:w-full object-cover rounded-md" />
            </div>
            <div class="flex">
                <div class="flex-1">
                    <Title size="lg">{{ spot?.title }}</Title>
                    <Text>{{ spot?.address?.formatted }}</Text>
                </div>
                <Money :cents="pricePerHourCents" suffix="/hr" size="lg" tone="primary" />
            </div>
            <!-- Only the picker lives here now. Paying is `/checkout`, a route of
                         its own, because a redirect payment method destroys this page. -->
            <div class="space-y-2">
                <Title weight="extrabold" class="text-[15px]">Pick your dates</Title>
                <Calendar v-if="calendarReady" multiple :model-value="(selectedDates as any)"
                    @update:model-value="onDatesChange" :min-value="minDate" :max-value="maxDate"
                    :is-date-disabled="isDateDisabled" class="bg-card rounded-lg border border-border" initial-focus />
                <!-- Its stand-in. One bar for the month heading and six rows at `aspect-7/1`
                     — a row seven cells wide and one tall, which is exactly what the real
                     grid's `aspect-square` cells add up to, so the height matches at any
                     screen width and nothing below it jumps when the calendar arrives.

                     Built from plain divs rather than the real template under
                     `data-loading`, which is the house style everywhere else: that trick
                     masks a template rendered over placeholder data, and the whole point
                     here is that the template is not mounted yet. Eight elements, against
                     the hundred this is standing in for. -->
                <div v-else class="rounded-lg border border-border bg-card p-3" aria-hidden="true">
                    <div class="mx-auto h-5 w-32 animate-pulse rounded-md bg-(--skeleton)"></div>
                    <!-- `mt-4` is the real gap under the heading; `pt-5` stands in for the
                         weekday row, which is a line of small text rather than a cell. -->
                    <div class="mt-4 pt-5">
                        <div v-for="row in 6" :key="row" class="aspect-7/1 animate-pulse rounded-md bg-(--skeleton)">
                        </div>
                    </div>
                </div>
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
                        <Surface v-for="(s, i) in activePicked" :key="`${s.start}-${s.end}-${i}`" variant="accent"
                            size="sm" orientation="horizontal" class="justify-between gap-2 text-sm font-semibold">
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
                                <Input type="time" step="1800" :min="win.start" :max="win.end" v-model="draft.start"
                                    class="flex-1" />
                                <ArrowRight class="text-muted-foreground size-4 shrink-0" />
                                <Input type="time" step="1800" :min="win.start" :max="win.end" v-model="draft.end"
                                    class="flex-1" />
                                <Button size="icon" :disabled="!slotValid(win, draft)" @click="addSlot(win, draft)">
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

            <!-- Which car. Required: the host has to know what is turning up,
                             and a plate typed here is saved to the renter's account on
                             submit, so the next booking form offers it. -->
            <div class="space-y-2">
                <div class="space-y-1">
                    <Title weight="extrabold" class="text-[15px]">Which car?</Title>
                    <Text>Pick one of your plates, or type a new one — we'll remember it.</Text>
                </div>
                <Combobox v-model:open="plateOpen" :ignore-filter="true" :reset-search-term-on-blur="false"
                    @update:model-value="pickPlate">
                    <ComboboxAnchor class="bg-card rounded-md">
                        <ComboboxInput v-model="plateTerm" placeholder="1-ABC-123" maxlength="16"
                            autocapitalize="characters" @input="plateOpen = true" />
                    </ComboboxAnchor>
                    <ComboboxList>
                        <ComboboxViewport>
                            <ComboboxItem v-for="p in plateMatches" :key="p" :value="p">
                                <Car class="size-4" />
                                {{ p }}
                            </ComboboxItem>
                            <ComboboxItem v-if="newPlate" :value="typedPlate">
                                <Plus class="size-4" />
                                Use "{{ typedPlate }}"
                            </ComboboxItem>
                            <Text v-if="!plateMatches.length && !newPlate" class="px-2 py-1.5">
                                {{ plates.length ? 'No plate like that.' : 'Type your plate.' }}
                            </Text>
                        </ComboboxViewport>
                    </ComboboxList>
                </Combobox>
                <Text v-if="newPlate && plate">
                    {{ plate }} will be saved to your account.
                </Text>
            </div>
        </template>

        <!-- Summary and button together: they are one strip, and it rises off the
                     bottom the moment there is a slot to pay for. Showing the bar and
                     enabling the button stay two questions — the plate is still required,
                     so a renter who has picked a time but no car sees the price and a
                     disabled button rather than nothing at all. -->
        <template #action>
            <div class="flex items-center justify-between pb-2.5 text-sm font-bold">
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
            <Button class="w-full h-11 font-bold" :disabled="!totals.slots || !plate || busy" @click="submit">
                Continue to payment
                <ArrowRight />
            </Button>
        </template>
    </DetailLayout>
</template>
