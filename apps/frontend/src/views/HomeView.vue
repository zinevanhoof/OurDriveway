<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { useQuery } from '@urql/vue';
import { formatCents } from '@/lib/money';
import { formatDay, formatSlots, isActiveNow, nextSlot, startOfWeek } from '@/lib/bookingDates';
import { locateUser, nearer, type Position } from '@/lib/geo';
import { recordId } from '@/lib/utils';
import { useAuthStore } from '@/stores/auth';
import { BOOKINGS_RENTED, WEEK_EARNINGS } from '@/api/graphql/booking';
import { FULL_SPOT, SPOTS_NEARBY } from '@/api/graphql/spot';
import SpotDetailDrawer from '@/components/spot/SpotDetailDrawer.vue';
import BookingFormComponent from '@/components/BookingFormComponent.vue';
import { CarFront, ChevronRight, CirclePlus, MapPin, Search, Star, Wallet } from '@lucide/vue';
import { imageUrl } from '@/lib/media'

const auth = useAuthStore()
const router = useRouter()

// Every query here waits on the session: main.ts rehydrates it asynchronously, and
// without this they all fire once with an undefined id first.
const paused = computed(() => !auth.user?.id)

// ─── the soonest booking ────────────────────────────────────────────────────
//
// ponytail: this pulls the renter's ENTIRE booking history to render one card. It
// is the same document and variables the bookings tab uses, so graphcache serves
// it without a second round trip — worth more than the rows saved. Give it its own
// `where: { ends_at: { gt: $now } }, limit: 1` document (booking_ends is already
// indexed) the day someone has hundreds of bookings.
const { data: bookings } = useQuery({
    query: BOOKINGS_RENTED,
    variables: computed(() => ({ renterId: auth.user?.id })),
    pause: paused,
})

// Confirmed only: a 'reserved' hold is an unfinished checkout, not somewhere you
// are due. `nextSlot` — not the booking's first slot — because a booking with an
// 09:00 and a 14:00 slot today has to read 14:00 once the morning is over.
//
// ponytail: candidates are ranked by wall-clock string, so two bookings in
// different zones within a day of each other can order wrong. Compare instants if
// this ever shows more than the single next one.
const next = computed(() => {
    const upcoming = (bookings.value?.bookings ?? [])
        .filter((b: any) => b?.status === 'confirmed')
        .map((b: any) => ({ booking: b, slot: nextSlot(b, b?.spot?.timezone) }))
        .filter((row: any) => row.slot !== null)
    return upcoming.sort((a: any, b: any) =>
        `${a.slot[0]}T${a.slot[1].start}`.localeCompare(`${b.slot[0]}T${b.slot[1].start}`),
    )[0] ?? null
})

const nextTimezone = computed(() => next.value?.booking?.spot?.timezone)
const happeningNow = computed(() => isActiveNow(next.value?.booking, nextTimezone.value))

// ─── earned this week ───────────────────────────────────────────────────────
//
// Read once at setup, not per render: the boundary only moves at Monday midnight,
// and a tab left open across it is not a case worth a timer.
const since = startOfWeek()

const { data: earnings } = useQuery({
    query: WEEK_EARNINGS,
    variables: computed(() => ({ ownerId: auth.user?.id, since })),
    pause: paused,
})

// ponytail: gross. No platform fee is modelled anywhere in the backend yet, so this
// is what renters paid, not what the host is owed. Subtract it here once one exists.
const earned = computed(() => earnings.value?.bookings_aggregate?.[0]?.amount_sum ?? 0)

// ─── nearby spots ───────────────────────────────────────────────────────────
const here = ref<Position | null>(null)
const locating = ref(false)

const locate = async () => {
    locating.value = true
    here.value = await locateUser()
    locating.value = false
}
onMounted(locate)

const { data: nearby } = useQuery({
    query: SPOTS_NEARBY,
    variables: computed(() => ({
        lng: here.value?.[0],
        lat: here.value?.[1],
        meters: 5000,
        me: auth.user?.id,
    })),
    pause: computed(() => here.value === null || !auth.user?.id),
})

// Sorted here because auto GraphQL cannot order by a function — `order` takes an
// enum of defined field names. Two cards out of one radius query is nothing to sort.
const nearest = computed(() => {
    const origin = here.value
    if (!origin) return []
    return [...(nearby.value?.spots ?? [])]
        .sort((a: any, b: any) =>
            nearer(origin, a.location.coordinates) - nearer(origin, b.location.coordinates))
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
const { data: selectedSpot, executeQuery: reexecuteSpot } = useQuery({
    query: FULL_SPOT,
    variables: computed(() => ({ id: recordId(selectedId.value) })),
    pause: computed(() => selectedId.value === null),
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
                <div class="text-xs text-muted-foreground font-medium">Earned this week</div>
                <div class="text-xl font-bold">{{ formatCents(earned) }}</div>
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
                    <img v-if="spot.images?.[0]" class="w-full h-28 object-cover" :src="imageUrl(spot.images[0])">
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
                        {{ formatCents(spot.price_per_hour) }}/hr
                    </div>
                </div>
            </div>
        </div>

        <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" :booking="selectedBooking"
            :bookable="!selectedBooking" @book="bookingOpen = true" />
        <BookingFormComponent v-model="bookingOpen" :spot="selectedSpot?.spot"
            @booked="() => reexecuteSpot({ requestPolicy: 'network-only' })" />
    </div>
</template>
