<script setup lang="ts">
// "My bookings": the renter's own bookings, under an upcoming/past tab, paged.
//
// Each row draws its spot from the card the booking carries — the one exception to a
// booking never carrying its spot — so the list is one request per page. Tapping a row
// opens the spot detail sheet for that booking.
import { computed, ref } from 'vue';
import { useInfiniteQuery, useQueryClient } from '@tanstack/vue-query';

import { fetchRenterBookings } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { Sentinel } from '@/components/base/sentinel';
import { isPlaceholder, placeholders, withLoadingRow } from '@/lib/placeholders';
import type { BookingScope } from '@/types/responses/view/HostBookingsPageResponse';
import RenterBookingRow from '@/components/booking/RenterBookingRow.vue';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Text } from '@/components/base/text';
import TabLayout from '@/components/layout/TabLayout.vue';

const queryClient = useQueryClient()

const scope = ref<BookingScope>('upcoming')

// `staleTime: 0`, and not as an optimisation to skip: a booking's status changes
// underneath this list constantly and never through anything this client did. A webhook
// confirms it, the expiry sweeper releases it, the host withdraws the spot — every one
// of those is an event on the log, so there is nothing here to invalidate on.
//
// Released holds are left out server-side: an abandoned checkout is noise, not a record.
const { data, fetchNextPage, hasNextPage, isFetchingNextPage, isPending } = useInfiniteQuery({
    queryKey: computed(() => viewKeys.renterBookings(scope.value)),
    queryFn: ({ pageParam }) =>
        fetchRenterBookings({
            scope: scope.value,
            status: ['reserved', 'confirmed', 'cancelled'],
            offset: pageParam,
        }),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
    staleTime: 0,
    placeholderData: placeholders.renterBookings,
})

const bookings = computed(() =>
    withLoadingRow(
        data.value?.pages.flatMap((p) => p.bookings) ?? [],
        isFetchingNextPage.value,
        placeholders.renterBookings.pages[0].bookings,
    ),
)

/** After a cancel, re-read past the projection rather than trusting the cache. */
const refresh = () => queryClient.invalidateQueries({ queryKey: viewKeys.bookings })
</script>

<template>
    <!-- No bottom padding: the list scrolls, so its `pb-3` is inside its own scroll
         box, and that runs flush to the navbar. -->
    <TabLayout title="My bookings">
        <Tabs v-model="scope">
            <TabsList class="w-full group-data-horizontal/tabs:h-10">
                <TabsTrigger value="upcoming" class="font-bold">Upcoming</TabsTrigger>
                <TabsTrigger value="past" class="font-bold">Past</TabsTrigger>
            </TabsList>
        </Tabs>

        <!-- The only part that scrolls; the two tab bars stay put. -->
        <div class="min-h-0 flex-1 space-y-2 overflow-y-auto no-scrollbar pb-3">
            <RenterBookingRow v-for="booking in bookings" :key="booking.id" :booking="booking" :past="scope === 'past'"
                :data-loading="isPlaceholder(booking.id)" @changed="refresh" />
            <Text v-if="!isPending && !bookings.length" size="sm" class="py-8 text-center">
                {{ scope === 'upcoming' ? 'Nothing booked yet.' : 'No past bookings.' }}
            </Text>

            <!-- Crossing this asks for the next page. -->
            <Sentinel :has-next-page="hasNextPage" :fetching="isFetchingNextPage" @load="fetchNextPage" />
        </div>
    </TabLayout>
</template>
