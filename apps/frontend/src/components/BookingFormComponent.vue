<script setup lang="ts">
import { useQuery } from "@urql/vue";
import { formatCents } from '@/lib/money';
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import FullScreenLayoutComponent from "./FullScreenLayoutComponent.vue";
import Calendar from "./ui/calendar/Calendar.vue";
import { ArrowRight, CalendarDays, Clock, Plus, X } from "@lucide/vue";
import Button from "./ui/button/Button.vue";
import Input from "./ui/input/Input.vue";
import Separator from "./ui/separator/Separator.vue";
import FilterChips from "./map/FilterChips.vue";

import { DateFormatter, DateValue, getLocalTimeZone, today } from "@internationalized/date";
import { computed, ref, watch } from "vue";

import { SPOT_BUSY } from "@/api/graphql/booking";
import { recordId } from "@/lib/utils";
import type { TimeSlot } from "@/types/domain/spot";
import type { BookingDraft } from "@/types/requests/BookingRequest";
import {
    type SpotAvailability,
    mergeBusy,
    remainingWindows,
    subtract,
    toMin,
} from "@/lib/bookingAvailability";

const props = defineProps<{
    spot?: {
        id: string;
        title: string;
        price_per_hour: number | string;
        address?: { formatted?: string };
        availability?: SpotAvailability;
    };
}>();

const open = defineModel<boolean>({ required: true });
const emit = defineEmits<{ submit: [draft: BookingDraft] }>();

// ─── Confirmed bookings → occupied slots to subtract from availability ───
const { data: bookingsData } = useQuery({
    query: SPOT_BUSY,
    variables: computed(() => ({ id: recordId(props.spot?.id) })),
    pause: computed(() => !props.spot?.id),
});
const occupied = computed(() => mergeBusy(bookingsData.value?.spotBusies ?? []));

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
// price_per_hour is EUR cents (integer) — see lib/money.ts.
const pricePerHourCents = computed(() => Number(props.spot?.price_per_hour) || 0);
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

// The drawer only hides on close (component stays mounted), so clear the picks
// when it closes — reopening starts fresh.
watch(open, (o) => {
    if (!o) {
        selectedDates.value = [];
        activeKey.value = "";
        pickedSlots.value = {};
        drafts.value = {};
    }
});

function submit() {
    if (!totals.value.slots || !props.spot) return;
    const booked: Record<string, TimeSlot[]> = {};
    for (const [k, arr] of Object.entries(pickedSlots.value)) if (arr.length) booked[k] = arr;
    emit("submit", { spotId: recordId(props.spot.id)!, booked, amountCents: totals.value.amountCents });
}
</script>

<template>
    <Drawer :open="open" :dismissible="false">
        <DrawerContent @close-auto-focus.prevent
            class="h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] [&>div:first-child]:hidden data-[vaul-drawer-direction=bottom]:mt-0 data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:max-h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:rounded-none z-50">
            <FullScreenLayoutComponent @close="open = false" :title="spot?.title ?? 'Book this spot'"
                :description="description">
                <template #main>
                    <div class="space-y-2">
                        <div class="text-[15px] font-extrabold">Pick your dates</div>
                        <Calendar multiple :model-value="(selectedDates as any)" @update:model-value="onDatesChange"
                            :min-value="minDate" :max-value="maxDate" :is-date-disabled="isDateDisabled"
                            class="bg-card rounded-lg border border-border" initial-focus />
                    </div>

                    <div v-if="!selectedDates?.length"
                        class="flex flex-col justify-center items-center gap-3 px-5 py-4.5 text-center bg-card rounded-lg border border-dashed border-border">
                        <div class="bg-accent text-accent-foreground p-2 rounded-md">
                            <CalendarDays />
                        </div>
                        <div class="w-3/4">Tap the highlighted dates the host has opened up, then choose your time
                            slots.</div>
                    </div>

                    <div v-else class="space-y-2">
                        <FilterChips :items="dateChips" :active="activeKey" @select="activeKey = $event"
                            @remove="(k) => selectedDates = selectedDates.filter(d => d.toString() !== k)" />
                        <div class="space-y-1">
                            <div class="text-[15px] font-extrabold">Choose your time</div>
                            <div class="text-xs text-muted-foreground font-medium">Add one or more 30-min time slots
                                within each of the host's open windows.</div>
                        </div>

                        <div v-if="activeDate" class="bg-card border border-border rounded-lg px-3.5 py-3.25 space-y-3">
                            <div class="flex justify-between items-end">
                                <div class="font-bold">{{ dfShort.format(activeDate.toDate(getLocalTimeZone())) }}</div>
                                <div class="text-xs font-bold text-primary">
                                    {{ activePicked.length ? `${activePicked.length} slot${activePicked.length > 1 ? 's'
                                    : ''}` : 'No time yet' }}
                                </div>
                            </div>

                            <!-- Already-picked slots for this date -->
                            <div v-if="activePicked.length" class="space-y-1.5">
                                <div v-for="(s, i) in activePicked" :key="`${s.start}-${s.end}-${i}`"
                                    class="flex items-center justify-between gap-2 bg-accent text-accent-foreground rounded-md px-2.5 py-1.5 text-sm font-semibold">
                                    <span>{{ s.start }} – {{ s.end }}</span>
                                    <span class="ml-auto text-xs text-muted-foreground">
                                        {{ slotHours(s) }} hr · {{ formatCents(slotCents(s)) }}
                                    </span>
                                    <button type="button" @click="removeSlot(i)"
                                        class="cursor-pointer opacity-70 hover:opacity-100">
                                        <X :size="14" />
                                    </button>
                                </div>
                            </div>

                            <Separator v-if="activePicked.length && windowRows.length" />

                            <!-- Open windows still available: add a slot inside each -->
                            <div v-if="windowRows.length" class="space-y-2">
                                <div v-for="{ win, key, draft } in windowRows" :key="key" class="space-y-1.5">
                                    <div class="flex items-center gap-1 text-xs text-muted-foreground font-bold">
                                        <Clock class="size-4" />
                                        Host open · {{ win.start }} - {{ win.end }}
                                    </div>
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
                            <div v-else-if="!activePicked.length" class="text-xs text-muted-foreground font-medium">
                                No open time left on this date.
                            </div>
                        </div>
                    </div>
                </template>

                <template #footer>
                    <div v-if="totals.slots" class="flex items-center justify-between pb-2.5 text-sm font-bold">
                        <span class="text-muted-foreground">
                            {{ totals.dates }} date{{ totals.dates > 1 ? 's' : '' }} ·
                            {{ totals.slots }} slot{{ totals.slots > 1 ? 's' : '' }} ·
                            {{ totals.hours }} hr{{ totals.hours !== 1 ? 's' : '' }}
                        </span>
                        <span>{{ formatCents(totals.amountCents) }}</span>
                    </div>
                    <Button class="w-full h-11 font-bold" :disabled="!totals.slots" @click="submit">
                        Continue to payment
                        <ArrowRight />
                    </Button>
                </template>
            </FullScreenLayoutComponent>
        </DrawerContent>
    </Drawer>
</template>
