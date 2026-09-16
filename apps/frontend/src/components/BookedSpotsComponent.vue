<script setup lang="ts">
// "My bookings": the renter's own bookings, under an upcoming/past tab, paged.
//
// Each row draws its spot from the card the booking carries — the one exception to a
// booking never carrying its spot — so the list is one request per page. Tapping a row
// opens the spot detail sheet for that booking.
import { computed, ref, useTemplateRef } from 'vue';
import { useInfiniteQuery, useQueryClient } from '@tanstack/vue-query';

import { fetchRenterBookings } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { useLoadMore } from '@/lib/loadMore';
import type { BookingScope } from '@/types/responses/view/HostBookingsPageResponse';
import BookedSpotRow from './BookedSpotRow.vue';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Text } from '@/components/base/text';

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
})

const bookings = computed(() => data.value?.pages.flatMap((p) => p.bookings) ?? [])

/** After a cancel, re-read past the projection rather than trusting the cache. */
const refresh = () => queryClient.invalidateQueries({ queryKey: viewKeys.bookings })

useLoadMore(useTemplateRef<HTMLElement>('sentinel'), { hasNextPage, isFetchingNextPage, fetchNextPage })
</script>

<template>
    <div class="space-y-2">
        <Tabs v-model="scope">
            <TabsList class="w-full group-data-horizontal/tabs:h-10">
                <TabsTrigger value="upcoming" class="font-bold">Upcoming</TabsTrigger>
                <TabsTrigger value="past" class="font-bold">Past</TabsTrigger>
            </TabsList>
        </Tabs>

        <BookedSpotRow v-for="booking in bookings" :key="booking.id" :booking="booking"
            :past="scope === 'past'" @changed="refresh" />
        <Text v-if="!isPending && !bookings.length" size="sm" class="py-8 text-center">
            {{ scope === 'upcoming' ? 'Nothing booked yet.' : 'No past bookings.' }}
        </Text>

        <!-- Crossing this asks for the next page. -->
        <div ref="sentinel" class="h-px"></div>
        <Text v-if="isFetchingNextPage" size="sm" class="pb-4 text-center">Loading…</Text>
    </div>
</template>
