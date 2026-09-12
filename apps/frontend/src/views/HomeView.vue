<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { useQuery, useQueryClient } from '@tanstack/vue-query';
import { formatCents } from '@/lib/money';
import { formatDay, formatSlots, isActiveNow, nextSlot } from '@/lib/bookingDates';
import { locateUser, nearer, type Position } from '@/lib/geo';
import { mergeBooked } from '@/lib/bookingAvailability';
import { useAuthStore } from '@/stores/auth';
import { fetchBalance, fetchNextBooking, fetchSpot, fetchSpotsNear } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import type { NextBookingResponse } from '@/types/responses/view/NextBookingResponse';
import type { TimeSlot } from '@/types/domain/spot';
import SpotDetailDrawer from '@/components/spot/SpotDetailDrawer.vue';
import BookingFormComponent from '@/components/BookingFormComponent.vue';
import { CarFront, ChevronRight, CirclePlus, MapPin, Search, Star, Wallet } from '@lucide/vue';
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { IconBox } from '@/components/base/icon-box';
import { Money } from '@/components/base/money';
import { SectionHeader } from '@/components/base/section-header';
import { Badge } from '@/components/ui/badge';

const auth = useAuthStore()
const router = useRouter()
const queryClient = useQueryClient()

// Every query here waits on the session: main.ts rehydrates it asynchronously, and
// without this they all fire once with an undefined id first.
const paused = computed(() => !auth.user?.id)

// ─── the soonest booking ────────────────────────────────────────────────────
//
// **One row, chosen by the server.** This used to fetch the renter's entire booking
// history, filter it to `confirmed` here, and rank what was left by comparing
// `"YYYY-MM-DDTHH:MM"` wall-clock strings — which orders wrong for two bookings in
// different zones within a day of each other. `/renter/bookings/next` sorts on `endsAt`,
// which is an instant, and `LIMIT 1` on `(renter_id, ends_at)` reads one row.
//
// `staleTime: 0` stays, and for the original reason: a booking's status moves through the
// event log, never through anything this client did, so there is nothing to invalidate on
// and a cached read would show a hold that lapsed or miss one a webhook just confirmed.
const { data: nextBooking } = useQuery({
    queryKey: viewKeys.nextBooking,
    queryFn: fetchNextBooking,
    staleTime: 0,
})

// `nextSlot` — not the booking's first slot — because a booking with an 09:00 and a 14:00
// slot today has to read 14:00 once the morning is over. It returns null when every slot
// has passed, which the server's `ends_at > now` makes rare but not impossible: the
// booking's last slot can be over while its `ends_at` is not.
type NextUp = { booking: NextBookingResponse; slot: [string, TimeSlot] }

const next = computed<NextUp | null>(() => {
    const booking = nextBooking.value
    if (!booking) return null
    const slot = nextSlot(booking, booking.spot?.timezone)
    return slot ? { booking, slot } : null
})

const nextTimezone = computed(() => next.value?.booking?.spot?.timezone)
const happeningNow = computed(() => isActiveNow(next.value?.booking, nextTimezone.value))

// ─── available to withdraw ──────────────────────────────────────────────────
//
// One endpoint answers this everywhere it appears — here and in the wallet — so the tile
// and the screen it opens cannot show two different numbers. It used to be a GraphQL
// aggregate over bookings windowed on `created_at`, which was the wrong field: when the
// booking was *made*, not when the money was earned.
const available = ref(0)
onMounted(async () => {
    if (paused.value) return
    try {
        available.value = (await fetchBalance()).availableCents
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
    <div class="py-2 px-4 space-y-4">
        <div class="grid grid-cols-2 gap-2">
            <Surface variant="primary" size="lg" class="gap-8" @click="router.push({ name: 'search' })">
                <Search />
                <div>
                    <Title tone="inverse">Find parking</Title>
                    <Text tone="inverse">Spots near you</Text>
                </div>
            </Surface>
            <Surface variant="elevated" size="lg" class="gap-8" @click="router.push({ name: 'spot-add' })">
                <CirclePlus class="text-primary" />
                <div>
                    <Title>Add a spot</Title>
                    <Text>Earn from your driveway</Text>
                </div>
            </Surface>
        </div>
        <Surface v-if="next" variant="elevated" orientation="horizontal" class="gap-3" @click="openBooking">
            <IconBox :tone="happeningNow ? 'primary' : 'accent'">
                <CarFront />
            </IconBox>
            <div class="flex-1">
                <Text size="eyebrow" weight="bold">
                    {{ happeningNow ? 'Happening now' : 'Upcoming booking' }}
                </Text>
                <Title weight="semibold">{{ next.booking.spot?.title }}</Title>
                <Text>
                    {{ formatDay(next.slot[0], nextTimezone) }} · {{ formatSlots([next.slot[1]]) }}
                </Text>
            </div>
            <ChevronRight class="text-muted-foreground" />
        </Surface>
        <Surface variant="elevated" orientation="horizontal" class="gap-3 bg-linear-135 from-accent to-card-2">
            <IconBox tone="primary">
                <Wallet />
            </IconBox>
            <div class="flex-1">
                <Text>Available to withdraw</Text>
                <Money :cents="available" size="xl" />
            </div>
            <Text size="sm" weight="semibold" tone="primary" class="flex items-center gap-1"
                @click="router.push({ name: 'wallet' })">
                Wallet
                <ChevronRight />
            </Text>
        </Surface>
        <div class="space-y-2">
            <SectionHeader>
                <Title>Nearby spots</Title>
                <template #action>
                    <Text weight="bold" tone="primary" @click="router.push({ name: 'search' })">See all</Text>
                </template>
            </SectionHeader>
            <!-- On native this really does re-prompt via Tauri's permission flow. On
                 web a hard-denied permission cannot be re-asked from script, and only
                 site settings can undo it — but the common case is a prompt that got
                 dismissed, and that one this fixes. The row is the message; no toast. -->
            <Surface v-if="!here" as="button" variant="elevated" orientation="horizontal" type="button" @click="locate"
                :disabled="locating" class="w-full gap-3 text-left">
                <IconBox>
                    <MapPin />
                </IconBox>
                <div class="flex-1">
                    <Title weight="semibold">{{ locating ? 'Finding you…' : 'Turn on location' }}</Title>
                    <Text>To see spots near you</Text>
                </div>
                <ChevronRight class="text-muted-foreground" />
            </Surface>
            <div v-else class="flex gap-2">
                <Surface v-for="spot in nearest" :key="spot.id" @click="openSpot(spot.id)" variant="elevated"
                    size="none" class="relative flex-1 min-w-0 overflow-hidden">
                    <img v-if="spot.images?.[0]" class="w-full h-28 object-cover" :src="spot.images[0]">
                    <div v-else class="w-full h-28 bg-accent"></div>
                    <div class="p-2">
                        <Title size="sm" weight="semibold" class="truncate">{{ spot.title }}</Title>
                        <!-- ponytail: hardcoded, because there is no rating in the
                             system to show. `rating` is declared on booking in both
                             schemas and nothing ever writes it: no BookingRated event,
                             no endpoint, no projector arm folding an average onto the
                             spot, no UI to submit one. That is the chain this needs. -->
                        <Text class="flex items-center gap-1">
                            <Star :size="16" />
                            4.9
                        </Text>
                    </div>
                    <Badge class="absolute left-2 top-2 rounded-sm">
                        {{ formatCents(spot.pricePerHour) }}/hr
                    </Badge>
                </Surface>
            </div>
        </div>

        <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" :booking="selectedBooking"
            :bookable="!selectedBooking" @book="bookingOpen = true" />
        <BookingFormComponent v-model="bookingOpen" :spot="selectedSpot"
            :booked="mergeBooked(selectedSpot?.bookings)"
            @booked="() => reexecuteSpot()" />
    </div>
</template>
