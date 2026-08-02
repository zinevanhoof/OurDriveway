<script setup lang="ts">
import { formatCents } from '@/lib/money';
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import FullScreenLayoutComponent from "./FullScreenLayoutComponent.vue";
import Calendar from "./ui/calendar/Calendar.vue";
import { ArrowRight, CalendarDays, Clock, Plus, ShieldCheck, Timer, X } from "@lucide/vue";
import Button from "./ui/button/Button.vue";
import Input from "./ui/input/Input.vue";
import Separator from "./ui/separator/Separator.vue";
import FilterChips from "./map/FilterChips.vue";
import { toast } from "vue-sonner";

import { DateFormatter, DateValue, getLocalTimeZone, today } from "@internationalized/date";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { recordId } from "@/lib/utils";
import * as bookingApi from "@/api/bookingApi";
import type { Reservation } from "@/api/bookingApi";
import type { TimeSlot } from "@/types/domain/spot";
import {
    type SpotAvailability,
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
        booked?: Record<string, TimeSlot[]>;
    };
}>();

const open = defineModel<boolean>({ required: true });
const emit = defineEmits<{ booked: [bookingId: string] }>();

// Slots already taken, straight off the spot. No reshaping and no filtering:
// everything in `booked` is taken, and a lapsed hold is removed server-side by
// the expiry sweeper rather than being filtered out here.
const occupied = computed(() => props.spot?.booked ?? {});

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

// ─── Checkout: hold the slots, then pay ───
// Reserving blocks the slots for everyone else while payment runs. The hold is
// what makes a slow card form safe; it lapses server-side if we never confirm.
const reservation = ref<Reservation | null>(null);
const busy = ref(false);
const remainingMs = ref(0);

const holdLabel = computed(() => {
    const total = Math.max(0, Math.ceil(remainingMs.value / 1000));
    return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
});

// One ticker for the whole component; it only does anything while a hold is live.
let ticker: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
    ticker = setInterval(() => {
        if (!reservation.value) return;
        remainingMs.value = new Date(reservation.value.expiresAt).getTime() - Date.now();
        if (remainingMs.value <= 0) {
            // The server has already freed these slots. Drop back to the picker
            // rather than letting them pay for something they no longer hold.
            reservation.value = null;
            toast.error("Your hold expired", {
                description: "Those times are back on the market. Please pick again.",
            });
        }
    }, 1000);
});
onBeforeUnmount(() => clearInterval(ticker));

async function submit() {
    if (!totals.value.slots || !props.spot || busy.value) return;
    const booked: Record<string, TimeSlot[]> = {};
    for (const [k, arr] of Object.entries(pickedSlots.value)) if (arr.length) booked[k] = arr;

    busy.value = true;
    try {
        reservation.value = await bookingApi.reserve({
            spotId: recordId(props.spot.id)!,
            booked,
            amountCents: totals.value.amountCents,
        });
        remainingMs.value = new Date(reservation.value.expiresAt).getTime() - Date.now();
    } catch (e: any) {
        // Someone took the slots between rendering and submitting, or the host
        // isn't open then. Either way the message names the offending slot.
        toast.error("Couldn't hold those times", { description: e.message });
    } finally {
        busy.value = false;
    }
}

async function pay() {
    if (!reservation.value || busy.value) return;
    busy.value = true;
    try {
        const id = reservation.value.id;
        await bookingApi.confirm(id);
        reservation.value = null;
        open.value = false;
        toast.success("Booked", { description: "Your parking spot is confirmed." });
        emit("booked", id);
    } catch (e: any) {
        toast.error("Payment couldn't be completed", { description: e.message });
    } finally {
        busy.value = false;
    }
}

/** Gives the slots back immediately instead of making the next renter wait. */
async function abandon() {
    const held = reservation.value;
    reservation.value = null;
    if (!held) return;
    try {
        await bookingApi.release(held.id);
    } catch {
        // Best effort — the server's expiry sweeper collects it either way.
    }
}

// The drawer only hides on close (component stays mounted), so clear the picks
// when it closes — reopening starts fresh. A live hold is released rather than
// silently left to expire.
watch(open, (o) => {
    if (!o) {
        void abandon();
        selectedDates.value = [];
        activeKey.value = "";
        pickedSlots.value = {};
        drafts.value = {};
    }
});

// The renter who closes the tab mid-checkout. `beforeunload` shows the browser's
// warning; `pagehide` fires whether or not they heed it, so the release goes out
// either way. Both are courtesies — the server-side sweeper is the guarantee.
function warnIfHolding(e: BeforeUnloadEvent) {
    if (reservation.value) e.preventDefault();
}
function releaseOnUnload() {
    if (reservation.value) void bookingApi.release(reservation.value.id, true).catch(() => { });
}
onMounted(() => {
    window.addEventListener("beforeunload", warnIfHolding);
    window.addEventListener("pagehide", releaseOnUnload);
});
onBeforeUnmount(() => {
    window.removeEventListener("beforeunload", warnIfHolding);
    window.removeEventListener("pagehide", releaseOnUnload);
});
</script>

<template>
    <Drawer :open="open" :dismissible="false">
        <DrawerContent @close-auto-focus.prevent
            class="h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] [&>div:first-child]:hidden data-[vaul-drawer-direction=bottom]:mt-0 data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:max-h-[calc(100dvh-var(--safe-top)-3.75rem-var(--safe-bottom))] data-[vaul-drawer-direction=bottom]:rounded-none z-50">
            <FullScreenLayoutComponent @close="open = false" :title="spot?.title ?? 'Book this spot'"
                :description="description">
                <template #main>
                    <!-- Checkout: the slots are held, the clock is running. -->
                    <div v-if="reservation" class="space-y-3">
                        <div
                            class="flex items-center gap-2.5 bg-card border border-border rounded-lg px-3.5 py-3.25">
                            <div class="bg-accent text-accent-foreground p-2 rounded-md">
                                <Timer class="size-5" />
                            </div>
                            <div class="space-y-0.5">
                                <div class="text-[15px] font-extrabold">Held for {{ holdLabel }}</div>
                                <div class="text-xs text-muted-foreground font-medium">
                                    We're holding these times while you pay. Leave now and they go back
                                    on the market.
                                </div>
                            </div>
                        </div>

                        <div class="bg-card border border-border rounded-lg px-3.5 py-3.25 space-y-2">
                            <div class="text-[15px] font-extrabold">Your booking</div>
                            <div v-for="date in sortedDates" :key="date.toString()" class="space-y-1">
                                <div class="text-xs text-muted-foreground font-bold">
                                    {{ dfShort.format(date.toDate(getLocalTimeZone())) }}
                                </div>
                                <div v-for="(s, i) in (pickedSlots[date.toString()] ?? [])"
                                    :key="`${s.start}-${s.end}-${i}`"
                                    class="flex items-center justify-between gap-2 bg-accent text-accent-foreground rounded-md px-2.5 py-1.5 text-sm font-semibold">
                                    <span>{{ s.start }} – {{ s.end }}</span>
                                    <span class="text-xs text-muted-foreground">
                                        {{ slotHours(s) }} hr · {{ formatCents(slotCents(s)) }}
                                    </span>
                                </div>
                            </div>
                            <Separator />
                            <!-- The server's figure, not the one the picker computed: it prices
                                 off the minutes it actually authorised, and that is what's charged. -->
                            <div class="flex items-center justify-between font-bold">
                                <span class="text-muted-foreground">Total</span>
                                <span>{{ formatCents(reservation.amountCents) }}</span>
                            </div>
                        </div>

                        <div
                            class="flex items-center gap-2 text-xs text-muted-foreground font-medium px-1">
                            <ShieldCheck class="size-4 shrink-0" />
                            No card is charged yet — payment isn't wired up.
                        </div>
                    </div>

                    <template v-else>
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
                </template>

                <template #footer>
                    <template v-if="reservation">
                        <Button class="w-full h-11 font-bold" :disabled="busy" @click="pay">
                            Pay {{ formatCents(reservation.amountCents) }}
                            <ArrowRight />
                        </Button>
                        <Button variant="ghost" class="w-full h-11 font-bold" :disabled="busy"
                            @click="abandon">
                            Back — release these times
                        </Button>
                    </template>
                    <template v-else>
                        <div v-if="totals.slots" class="flex items-center justify-between pb-2.5 text-sm font-bold">
                            <span class="text-muted-foreground">
                                {{ totals.dates }} date{{ totals.dates > 1 ? 's' : '' }} ·
                                {{ totals.slots }} slot{{ totals.slots > 1 ? 's' : '' }} ·
                                {{ totals.hours }} hr{{ totals.hours !== 1 ? 's' : '' }}
                            </span>
                            <span>{{ formatCents(totals.amountCents) }}</span>
                        </div>
                        <Button class="w-full h-11 font-bold" :disabled="!totals.slots || busy" @click="submit">
                            Continue to payment
                            <ArrowRight />
                        </Button>
                    </template>
                </template>
            </FullScreenLayoutComponent>
        </DrawerContent>
    </Drawer>
</template>
