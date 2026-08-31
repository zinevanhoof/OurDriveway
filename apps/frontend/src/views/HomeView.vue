<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { useQuery, useQueryClient } from '@tanstack/vue-query';
import { formatCents } from '@/lib/money';
import { formatDay, formatSlots, isActiveNow, nextSlot } from '@/lib/bookingDates';
import { locateUser, nearer, type Position } from '@/lib/geo';
import { mergeBooked } from '@/lib/bookingAvailability';
import { useAuthStore } from '@/stores/auth';
import * as paymentApi from '@/api/paymentApi';
import { fetchMyBookings, fetchSpot, fetchSpotsNear, viewKeys } from '@/api/viewApi';
import type { BookingListItem } from '@/types/view';
import type { TimeSlot } from '@/types/domain/spot';
import SpotDetailDrawer from '@/components/spot/SpotDetailDrawer.vue';
import BookingFormComponent from '@/components/BookingFormComponent.vue';
import { CarFront, ChevronRight, CirclePlus, MapPin, Search, Star, Wallet } from '@lucide/vue';

const auth = useAuthStore()
const router = useRouter()
const queryClient = useQueryClient()

// Every query here waits on the session: main.ts rehydrates it asynchronously, and
// without this they all fire once with an undefined id first.
const paused = computed(() => !auth.user?.id)

// ─── the soonest booking ────────────────────────────────────────────────────
//
// `network-only` for the same reason the bookings tab is: statuses move through the
// event log, never through a GraphQL mutation, so graphcache has nothing to invalidate
// on and a cached read would show a hold that lapsed or miss a booking a webhook just
// confirmed. This card saying something different from the tab is worse than the extra
// request.
//
// ponytail: this pulls the renter's ENTIRE booking history to render one card. It shares
// the document and variables with the bookings tab, so the two still share one cache
// entry — they just no longer share one *request*. Give it its own
// `where: { ends_at: { gt: $now } }, limit: 1` document (booking_ends is already
// indexed) the day someone has hundreds of bookings.
const { data: bookings } = useQuery({
    queryKey: viewKeys.myBookings,
    queryFn: fetchMyBookings,
    staleTime: 0,
})

// Confirmed only: a 'reserved' hold is an unfinished checkout, not somewhere you
// are due. `nextSlot` — not the booking's first slot — because a booking with an
// 09:00 and a 14:00 slot today has to read 14:00 once the morning is over.
//
// ponytail: candidates are ranked by wall-clock string, so two bookings in
// different zones within a day of each other can order wrong. Compare instants if
// this ever shows more than the single next one.
// The `slot !== null` filter needs a type predicate to narrow, which it did not while
// the query result was `any` — every row here was untyped, so the template read
// `next.slot[0]` off something the compiler knew nothing about. `nextSlot` returns null
// for a booking whose slots have all passed, and that was always reachable; it just was
// not visible until the responses acquired types.
type NextUp = { booking: BookingListItem; slot: [string, TimeSlot] }

const next = computed<NextUp | null>(() => {
    const upcoming = (bookings.value ?? [])
        .filter((b) => b.status === 'confirmed')
        .map((b) => ({ booking: b, slot: nextSlot(b, b.spot?.timezone) }))
        .filter((row): row is NextUp => row.slot !== null)
    return upcoming.sort((a, b) =>
        `${a.slot[0]}T${a.slot[1].start}`.localeCompare(`${b.slot[0]}T${b.slot[1].start}`),
    )[0] ?? null
})

const nextTimezone = computed(() => next.value?.booking?.spot?.timezone)
const happeningNow = computed(() => isActiveNow(next.value?.booking, nextTimezone.value))

// ─── available to withdraw ──────────────────────────────────────────────────
//
// From payment-service, not from the booking read model. It used to be a GraphQL
// aggregate over bookings windowed on `created_at` — which was the wrong field (when
// the booking was *made*, not when the money was earned) and, worse, a second answer to
// "what have I earned" sitting next to a withdraw button that spends the first one.
//
// One source now: the same endpoint the payout section reads, so the figure here and
// the amount that button withdraws cannot disagree.
const available = ref(0)
onMounted(async () => {
    if (paused.value) return
    try {
        available.value = (await paymentApi.earnings()).availableCents
    } catch {
        // A tile that can't load its number shows zero rather than breaking the screen.
    }
})

// ─── nearby spots ───────────────────────────────────────────────────────────
const here = ref<Position | null>(null)
const locating = ref(false)

const locate = async () => {
    locating.value = true
    here.value = await locateUser()
    locating.value = false
}
onMounted(locate)

// The `me` variable is gone: excluding the caller's own spots is unconditional
// server-side now, so this and the map's radius query differ in nothing but the radius
// — which is why they share one endpoint and one key shape.
const { data: nearby } = useQuery({
    queryKey: computed(() => viewKeys.nearby(here.value?.[0] ?? 0, here.value?.[1] ?? 0, 5000)),
    queryFn: () => fetchSpotsNear(here.value![0], here.value![1], 5000),
    enabled: computed(() => here.value !== null),
})

// Still sorted client-side, and still for a reason: ordering by distance would mean an
// ORDER BY over the same haversine the WHERE already computes, on a result the caller
// then truncates to two. Two cards out of one radius query is nothing to sort.
const nearest = computed(() => {
    const origin = here.value
    if (!origin) return []
    return [...(nearby.value ?? [])]
        .sort((a, b) => nearer(origin, [a.lng, a.lat]) - nearer(origin, [b.lng, b.lat]))
        .slice(0, 2)
})

// The same sheet the map opens. It owns its own query off the id, so tapping a card
// only has to say which spot.
const selectedId = ref<string | null>(null)
const detailOpen = ref(false)
const bookingOpen = ref(false)

// Set only when the sheet was opened from the upcoming-booking card. It is what
// tells the drawer to list that booking's schedule and to drop the book button —
// you cannot book a spot you have already booked. Same two modes as the map and the
// bookings list, one drawer instead of two.
const selectedBooking = ref<any>(null)

const openSpot = (id: string) => {
    selectedBooking.value = null
    selectedId.value = id
    detailOpen.value = true
}

// Feeds the booking form the drawer hands off to. urql dedupes it against the
// drawer's identical query, so this is still one request.
//
// `network-only` because this is the one query someone books against, and cached
// availability is stale by construction: every write in this app goes through REST, so
// there are no GraphQL mutations for graphcache to invalidate on. The list queries stay
// cached and re-run on every pan.
//
// Freshness, not correctness. A row that landed a moment ago can already be wrong; the
// authority is the server's availability check, published under compare-and-swap. This
// only stops the picker offering slots it then has to retract.
const { data: selectedSpot } = useQuery({
    queryKey: computed(() => viewKeys.spot(selectedId.value ?? '')),
    queryFn: () => fetchSpot(selectedId.value!),
    enabled: computed(() => selectedId.value !== null),
    staleTime: 0,
})

const reexecuteSpot = () =>
    queryClient.invalidateQueries({ queryKey: viewKeys.spot(selectedId.value ?? '') })

// `staleTime: 0` alone is not enough: `selectedId` is never cleared on close, so
// reopening the *same* spot changes neither the key nor the enabled state, and a query
// that is already mounted does not refetch on its own — the picker would keep whatever
// it read the first time, including slots this renter has since held and abandoned.
// Opening the form is therefore an explicit invalidation. (Same reasoning as under
// urql, where the equivalent was that neither the variables nor the pause changed.)
watch(bookingOpen, (isOpen) => {
    if (isOpen) void reexecuteSpot()
})

const openBooking = () => {
    selectedBooking.value = next.value?.booking ?? null
    selectedId.value = next.value?.booking?.spot?.id ?? null
    detailOpen.value = true
}
</script>

<template>
    <!-- ponytail: the tap targets here are divs, so none of them are keyboard
         reachable and a screen reader announces no action. ProfileView's
         `<button class="flex … w-full text-left">` is the shape that fixes it
         without disturbing layout — worth doing to the whole screen at once. -->
    <div class="pt-2 px-4 pb-2 space-y-4">
        <div class="flex gap-2">
            <div class="flex-1 bg-primary text-primary-foreground rounded-md p-4 space-y-8"
                @click="router.push({ name: 'search' })">
                <Search />
                <div>
                    <div class="font-bold">Find parking</div>
                    <div class="text-xs font-medium">Spots near you</div>
                </div>
            </div>
            <div class="flex-1 bg-card text-card-foreground rounded-md p-4 space-y-8 border border-border shadow-xs"
                @click="router.push({ name: 'spot-add' })">
                <CirclePlus class="text-primary" />
                <div>
                    <div class="font-bold">Add a spot</div>
                    <div class="text-xs text-muted-foreground font-medium">Earn from your driveway</div>
                </div>
            </div>
        </div>
        <div v-if="next"
            class="flex items-center gap-3 bg-card text-card-foreground rounded-md border border-border shadow-xs p-3"
            @click="openBooking">
            <div class="p-2 rounded-md"
                :class="happeningNow ? 'bg-primary text-primary-foreground' : 'bg-accent text-accent-foreground'">
                <CarFront :size="22" />
            </div>
            <div class="flex-1">
                <div class="text-[10px] text-muted-foreground font-bold">
                    {{ happeningNow ? 'HAPPENING NOW' : 'UPCOMING BOOKING' }}
                </div>
                <div class="font-semibold">{{ next.booking.spot?.title }}</div>
                <div class="text-xs text-muted-foreground font-medium">
                    {{ formatDay(next.slot[0], nextTimezone) }} · {{ formatSlots([next.slot[1]]) }}
                </div>
            </div>
            <ChevronRight class="text-muted-foreground" />
        </div>
        <div
            class="flex items-center gap-3 bg-linear-135 from-accent to-card-2 rounded-md border border-border shadow-xs p-3">
            <div class="p-2 rounded-md bg-primary text-primary-foreground">
                <Wallet :size="22" />
            </div>
            <div class=flex-1>
                <div class="text-xs text-muted-foreground font-medium">Available to withdraw</div>
                <div class="text-xl font-bold">{{ formatCents(available) }}</div>
            </div>
            <div class="flex items-center gap-1 text-primary text-sm font-semibold"
                @click="router.push({ name: 'spots' })">
                Manage
                <ChevronRight />
            </div>
        </div>
        <div class="space-y-2">
            <div class="flex justify-between items-end">
                <div class="font-bold">Nearby spots</div>
                <div class="text-primary text-xs font-bold" @click="router.push({ name: 'search' })">See all</div>
            </div>
            <!-- On native this really does re-prompt via Tauri's permission flow. On
                 web a hard-denied permission cannot be re-asked from script, and only
                 site settings can undo it — but the common case is a prompt that got
                 dismissed, and that one this fixes. The row is the message; no toast. -->
            <button v-if="!here" type="button" @click="locate" :disabled="locating"
                class="flex gap-3 items-center p-3 w-full text-left bg-card border border-border rounded-md shadow-xs">
                <div class="p-2 rounded-md bg-accent text-accent-foreground">
                    <MapPin :size="22" />
                </div>
                <div class="flex-1">
                    <div class="font-semibold">{{ locating ? 'Finding you…' : 'Turn on location' }}</div>
                    <div class="text-xs text-muted-foreground font-medium">To see spots near you</div>
                </div>
                <ChevronRight class="text-muted-foreground" />
            </button>
            <div v-else class="flex gap-2">
                <div v-for="spot in nearest" :key="spot.id" @click="openSpot(spot.id)"
                    class="relative flex-1 min-w-0 bg-card border border-border shadow-xs rounded-md overflow-hidden">
                    <img v-if="spot.images?.[0]" class="w-full h-28 object-cover" :src="spot.images[0]">
                    <div v-else class="w-full h-28 bg-accent"></div>
                    <div class="p-2">
                        <div class="text-sm font-semibold truncate">{{ spot.title }}</div>
                        <!-- ponytail: hardcoded, because there is no rating in the
                             system to show. `rating` is declared on booking in both
                             schemas and nothing ever writes it: no BookingRated event,
                             no endpoint, no projector arm folding an average onto the
                             spot, no UI to submit one. That is the chain this needs. -->
                        <div class="flex items-center gap-1 text-xs text-muted-foreground font-medium">
                            <Star :size="16" />
                            4.9
                        </div>
                    </div>
                    <div
                        class="absolute left-2 top-2 bg-primary text-primary-foreground text-xs font-bold rounded-sm px-2 py-0.5">
                        {{ formatCents(spot.pricePerHour) }}/hr
                    </div>
                </div>
            </div>
        </div>

        <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" :booking="selectedBooking"
            :bookable="!selectedBooking" @book="bookingOpen = true" />
        <BookingFormComponent v-model="bookingOpen" :spot="selectedSpot"
            :booked="mergeBooked(selectedSpot?.bookings)"
            @booked="() => reexecuteSpot()" />
    </div>
</template>
