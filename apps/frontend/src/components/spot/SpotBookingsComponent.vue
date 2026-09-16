<script setup lang="ts">
// Every booking on one spot, for its host — the screen behind the manage screen's
// two-row preview.
//
// Same endpoint as the preview, which asks it for two confirmed rows. This one pages
// through it twenty at a time, under two tab rows: upcoming/past and confirmed/cancelled.
import { computed, ref, useTemplateRef, watch } from 'vue'
import { useInfiniteQuery, useQuery } from '@tanstack/vue-query'
import { useIntersectionObserver } from '@vueuse/core'
import { useRouter } from 'vue-router'
import { Car } from '@lucide/vue'

import FullScreenLayoutComponent from '@/components/FullScreenLayoutComponent.vue'
import HostBookingDrawer from '@/components/spot/HostBookingDrawer.vue'
import Avatar from '@/components/ui/avatar/Avatar.vue'
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue'
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue'
import Button from '@/components/ui/button/Button.vue'
import { Badge } from '@/components/ui/badge'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Surface } from '@/components/base/surface'
import { Text } from '@/components/base/text'
import { Money } from '@/components/base/money'

import { fetchHostSpot, fetchHostSpotBookings } from '@/api/viewApi'
import { viewKeys } from '@/api/keys'
import type { ApiError } from '@/api/client'
import type { BookingScope } from '@/types/responses/view/HostBookingsPageResponse'
import type { HostBookingListItemResponse } from '@/types/responses/view/HostBookingListItemResponse'
import { formatDay, formatSlots, sortedDays } from '@/lib/bookingDates'

const { id } = defineProps<{ id: string }>()

const router = useRouter()

// The same key the manage screen filled a moment ago, so the title and the zone are
// already in the cache and this screen opens without waiting on anything.
const { data: spot } = useQuery({
    queryKey: viewKeys.hostSpot(id),
    queryFn: () => fetchHostSpot(id),
})
const timezone = computed(() => spot.value?.timezone)

const scope = ref<BookingScope>('upcoming')
const status = ref<'confirmed' | 'cancelled'>('confirmed')

// One query, not four. Both tabs are part of the key, so switching is a different
// cached list rather than a second copy of this block — and going back to a tab shows
// the pages already fetched for it.
const { data, fetchNextPage, hasNextPage, isFetchingNextPage, isPending, isError, error, refetch } =
    useInfiniteQuery({
        queryKey: computed(() => viewKeys.hostSpotBookings(id, scope.value, status.value)),
        queryFn: ({ pageParam }) =>
            fetchHostSpotBookings(id, { scope: scope.value, status: [status.value], offset: pageParam }),
        initialPageParam: 0,
        // Null ends the list. The server says which offset is next; nothing here counts.
        getNextPageParam: (last) => last.nextOffset ?? undefined,
    })

const bookings = computed(() => data.value?.pages.flatMap((p) => p.bookings) ?? [])
// `total` is on every page and nothing here reads it: the screen scrolls rather than
// counting. It stays on the response because the server counts anyway — `nextOffset` is
// derived from it — and a tab that one day wants "Upcoming (7)" already has the number.

// ─── one booking ────────────────────────────────────────────────────────────

const selected = ref<HostBookingListItemResponse | null>(null)
const detailOpen = ref(false)

const openBooking = (booking: HostBookingListItemResponse) => {
    selected.value = booking
    detailOpen.value = true
}

/** "Mon, Aug 3 · 09:00–10:00", plus a count when the booking spans more days. */
const when = (booking: HostBookingListItemResponse) => {
    const days = sortedDays(booking)
    if (!days.length) return ''
    const [date, slots] = days[0]
    const rest = days.length - 1
    return `${formatDay(date, timezone.value)} · ${formatSlots(slots)}`
        + (rest ? ` +${rest} more ${rest === 1 ? 'day' : 'days'}` : '')
}

/** Only the rows that are not a plain paid booking say anything. */
const badge = (status: string) => {
    switch (status) {
        case 'reserved': return { label: 'Awaiting payment', variant: 'secondary' } as const
        case 'cancelled': return { label: 'Cancelled', variant: 'destructive' } as const
        case 'released': return { label: 'Expired', variant: 'outline' } as const
        default: return null
    }
}

// ─── infinite scroll ────────────────────────────────────────────────────────
//
// The observer records whether the sentinel is on screen; the watcher decides whether to
// ask for another page. Split for the reason spelled out in `WalletComponent`: an
// IntersectionObserver reports *transitions*, and a first page that does not fill the
// screen leaves the sentinel visible with no further callback ever coming.
const sentinel = useTemplateRef<HTMLElement>('sentinel')
const sentinelVisible = ref(false)
useIntersectionObserver(sentinel, ([entry]) => {
    sentinelVisible.value = !!entry?.isIntersecting
})

watch([sentinelVisible, hasNextPage, isFetchingNextPage], () => {
    if (sentinelVisible.value && hasNextPage.value && !isFetchingNextPage.value) {
        void fetchNextPage()
    }
}, { immediate: true })
</script>

<template>
    <FullScreenLayoutComponent @close="router.back()" title="Bookings" :description="spot?.title">
        <template #main>
            <Tabs v-model="scope">
                <TabsList class="w-full">
                    <TabsTrigger value="upcoming">Upcoming</TabsTrigger>
                    <TabsTrigger value="past">Past</TabsTrigger>
                </TabsList>
            </Tabs>
            <Tabs v-model="status">
                <TabsList class="w-full">
                    <TabsTrigger value="confirmed">Confirmed</TabsTrigger>
                    <TabsTrigger value="cancelled">Cancelled</TabsTrigger>
                </TabsList>
            </Tabs>

            <div v-auto-animate class="space-y-2">
                <Surface v-for="booking in bookings" :key="booking.id" orientation="horizontal" class="gap-2"
                    interactive @click="openBooking(booking)">
                    <Avatar size="lg">
                        <AvatarImage v-if="booking.renter?.profilePicture" :src="booking.renter.profilePicture" />
                        <AvatarFallback :name="{
                            firstName: booking.renter?.firstName ?? '',
                            lastName: booking.renter?.lastName ?? '',
                        }" />
                    </Avatar>
                    <div class="flex-1 min-w-0">
                        <Text size="sm" tone="default" class="flex items-center gap-1.5">
                            {{ booking.renter ? `${booking.renter.firstName} ${booking.renter.lastName}` : 'A renter' }}
                            <Badge v-if="badge(booking.status)" :variant="badge(booking.status)!.variant">
                                {{ badge(booking.status)!.label }}
                            </Badge>
                        </Text>
                        <Text class="truncate">{{ when(booking) }}</Text>
                        <Text class="flex items-center gap-1">
                            <Car :size="14" />
                            {{ booking.licensePlate }}
                        </Text>
                    </div>
                    <Money :cents="booking.amount" signed size="md" />
                </Surface>
            </div>

            <Text v-if="isPending" size="sm" class="py-6 text-center">Loading…</Text>
            <!-- A failed read must not render as an empty list: "nothing is booked" is
                 the one wrong thing this screen can tell a host. -->
            <div v-else-if="isError" class="space-y-2 py-6 text-center">
                <Text size="sm">{{ (error as ApiError).detail.join(' ') }}</Text>
                <Button variant="outline" size="sm" @click="() => refetch()">Try again</Button>
            </div>
            <Text v-else-if="!bookings.length" size="sm" class="py-6 text-center">
                {{ status === 'cancelled'
                    ? (scope === 'upcoming' ? 'No upcoming bookings were cancelled.' : 'No past bookings were cancelled.')
                    : (scope === 'upcoming' ? 'Nothing booked yet.' : 'No bookings have finished yet.') }}
            </Text>

            <!-- Crossing this asks for the next page. -->
            <div ref="sentinel" class="h-px"></div>
            <Text v-if="isFetchingNextPage" size="sm" class="pb-4 text-center">Loading…</Text>
        </template>
    </FullScreenLayoutComponent>

    <HostBookingDrawer v-model:open="detailOpen" :booking="selected" :timezone="timezone" />
</template>
