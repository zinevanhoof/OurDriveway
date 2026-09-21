<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { useQuery } from '@tanstack/vue-query';
import { formatCents } from '@/lib/money';
import { formatDay, formatSlots, isActiveNow, nextSlot } from '@/lib/bookingDates';
import { locateUser, nearer, type Position } from '@/lib/geo';
import { useAuthStore } from '@/stores/auth';
import { fetchBalance, fetchNextBooking, fetchNotifications, fetchSpotsNear } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { isPlaceholder, placeholders } from '@/lib/placeholders';
import type { NextBookingResponse } from '@/types/responses/view/NextBookingResponse';
import type { TimeSlot } from '@/types/domain/spot';
import SpotDetailDrawer from '@/components/spot/SpotDetailDrawer.vue';
import SpotRating from '@/components/spot/SpotRating.vue';
import { Bell, CarFront, ChevronRight, CirclePlus, MapPin, Search, Wallet } from '@lucide/vue';
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { IconBox } from '@/components/base/icon-box';
import { Money } from '@/components/base/money';
import { SectionHeader } from '@/components/base/section-header';
import { Badge } from '@/components/ui/badge';
import Avatar from '@/components/ui/avatar/Avatar.vue';
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue';
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue';
import TabLayout from '@/components/layout/TabLayout.vue';

const auth = useAuthStore()
const router = useRouter()

// Every query here waits on the session: main.ts rehydrates it asynchronously, and
// without this they all fire once with an undefined id first.
const paused = computed(() => !auth.user?.id)

// ─── the header ─────────────────────────────────────────────────────────────

// Polled: a rating prompt comes due when a booking ends, and a minute late is fine.
const { data: notifications } = useQuery({
    queryKey: viewKeys.notifications,
    queryFn: fetchNotifications,
    refetchInterval: 60_000,
})
const unseen = computed(() => notifications.value?.filter((n) => !n.seen).length ?? 0)

const greeting = computed(() => {
    const h = new Date().getHours()
    if (h < 12) return "Good morning"
    if (h < 18) return "Good afternoon"
    return "Good evening"
})

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

// The card's title and zone come from `spot` on the booking — a renter's booking is the
// one exception that carries a card of its spot.
//
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

const nextSpot = computed(() => next.value?.booking.spot)
const nextTimezone = computed(() => nextSpot.value?.timezone)
const happeningNow = computed(() => isActiveNow(next.value?.booking, nextTimezone.value))

// ─── available to withdraw ──────────────────────────────────────────────────
//
// One endpoint answers this everywhere it appears — here and in the wallet — so the tile
// and the screen it opens cannot show two different numbers. It used to be a GraphQL
// aggregate over bookings windowed on `created_at`, which was the wrong field: when the
// booking was *made*, not when the money was earned.
//
// The wallet's own query and key, so the two share one cache. A failed read leaves the
// placeholder's `isPlaceholderData` false and `data` undefined, which shows zero rather
// than breaking the screen — what the hand-rolled fetch here used to do on purpose.
const { data: balance, isPlaceholderData: balanceLoading } = useQuery({
    queryKey: viewKeys.balance,
    queryFn: fetchBalance,
    enabled: computed(() => !paused.value),
    placeholderData: placeholders.balance,
})
const available = computed(() => balance.value?.availableCents ?? 0)

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
    placeholderData: placeholders.nearby,
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

// Set only when the sheet was opened from the upcoming-booking card. It is what
// tells the drawer to read the spot as the renter's, list that booking's schedule and
// drop the book button — you cannot book a spot you have already booked.
const selectedBooking = ref<NextBookingResponse | null>(null)

const openSpot = (id: string) => {
    selectedBooking.value = null
    selectedId.value = id
    detailOpen.value = true
}

const openBooking = () => {
    selectedBooking.value = next.value?.booking ?? null
    selectedId.value = next.value?.booking?.spotId ?? null
    detailOpen.value = true
}
</script>

<template>
    <!-- ponytail: the tap targets here are divs, so none of them are keyboard
         reachable and a screen reader announces no action. ProfileView's
         `<button class="flex … w-full text-left">` is the shape that fixes it
         without disturbing layout — worth doing to the whole screen at once. -->
    <TabLayout>
        <!-- Not a page name but a greeting, so the slot rather than the `title` prop. -->
        <template #title>
            <div>
                <Text size="sm" weight="normal">{{ greeting }}</Text>
                <Title size="2xl" weight="extrabold">{{ auth.user?.firstName }} {{ auth.user?.lastName }}</Title>
            </div>
        </template>
        <template #actions>
            <button type="button" class="relative" :aria-label="`Notifications, ${unseen} new`"
                @click="router.push({ name: 'notifications' })">
                <IconBox size="lg" tone="card" shape="circle">
                    <Bell />
                </IconBox>
                <span v-if="unseen"
                    class="absolute -right-1 -top-1 flex min-w-5 h-5 items-center justify-center rounded-full bg-destructive px-1 text-xs font-semibold text-white">
                    {{ unseen > 9 ? '9+' : unseen }}
                </span>
            </button>
            <Avatar @click="router.push({ name: 'profile' })" size="lg">
                <AvatarImage v-if="auth.user?.profilePicture" :src="auth.user.profilePicture" />
                <AvatarFallback
                    :name="{ firstName: auth.user?.firstName ?? '', lastName: auth.user?.lastName ?? '' }" />
            </Avatar>
        </template>
        <!-- Everything under the header scrolls, so the header stays put. -->
        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto no-scrollbar pb-3">
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
                    <Title weight="semibold">{{ nextSpot?.title }}</Title>
                    <Text>
                        {{ formatDay(next.slot[0], nextTimezone) }} · {{ formatSlots([next.slot[1]]) }}
                    </Text>
                </div>
                <ChevronRight class="text-muted-foreground" />
            </Surface>
            <Surface variant="elevated" orientation="horizontal"
                class="gap-3 cursor-pointer bg-linear-135 from-accent to-card-2"
                @click="router.push({ name: 'wallet' })">
                <IconBox tone="primary">
                    <Wallet />
                </IconBox>
                <div class="flex-1">
                    <Text>Available to withdraw</Text>
                    <Money :cents="available" size="xl" :data-loading="balanceLoading" />
                </div>
                <Text size="sm" weight="semibold" tone="primary" class="flex items-center gap-1">
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
                <Surface v-if="!here" as="button" variant="elevated" orientation="horizontal" type="button"
                    @click="locate" :disabled="locating" class="w-full gap-3 text-left">
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
                        size="none" class="relative flex-1 min-w-0 overflow-hidden"
                        :data-loading="isPlaceholder(spot.id)">
                        <img v-if="spot.images?.[0]" class="w-full h-28 object-cover" :src="spot.images[0]">
                        <div v-else class="w-full h-28 bg-accent"></div>
                        <div class="p-2">
                            <Title size="sm" weight="semibold" class="truncate">{{ spot.title }}</Title>
                            <!-- Nothing until somebody rates the spot. Nothing writes a rating
                                 yet: that needs a BookingRated event, an endpoint and a UI. -->
                            <SpotRating :spot-id="spot.id" />
                        </div>
                        <Badge class="absolute left-2 top-2 rounded-sm">
                            {{ formatCents(spot.pricePerHour) }}/hr
                        </Badge>
                    </Surface>
                </div>
            </div>
        </div>
    </TabLayout>

    <SpotDetailDrawer v-model:open="detailOpen" :spot-id="selectedId" :booking="selectedBooking"
        :renter="!!selectedBooking" :bookable="!selectedBooking"
        @book="router.push({ name: 'book', params: { id: selectedId } })" />
</template>
