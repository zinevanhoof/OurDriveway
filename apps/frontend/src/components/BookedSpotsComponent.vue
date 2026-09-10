<script setup lang="ts">
import { fetchRenterBookings, viewKeys } from '@/api/viewApi';
import type { RenterBookingResponse } from '@/types/view';
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'
import { useQuery, useQueryClient } from '@tanstack/vue-query';
import { computed } from 'vue';
import BookedSpotRow from './BookedSpotRow.vue';
import { Text } from '@/components/base/text';

const queryClient = useQueryClient()

// `staleTime: 0`, and not as an optimisation to skip: a booking's status changes
// underneath this list constantly and never through anything this client did. A webhook
// confirms it, the expiry sweeper releases it, the host withdraws the spot — every one
// of those is an event on the log, so there is nothing here to invalidate on and a
// cached list shows holds that lapsed hours ago.
//
// The `renterId` variable and the `pause` that guarded it are gone: the server takes the
// renter from the token, so there is no id to wait for the session to rehydrate.
const { data } = useQuery({
    queryKey: viewKeys.renterBookings,
    queryFn: fetchRenterBookings,
    staleTime: 0,
})

// A booking that ended without happening is not history the renter wants a tab
// full of — an abandoned checkout in particular is noise, not a record. The one
// exception is a booking the *host* withdrew: that one has to stay visible, or a
// trip someone paid for just disappears without ever saying why.
const live = computed(() =>
    (data.value ?? []).filter(
        (b) =>
            b.status !== 'released' &&
            (b.status !== 'cancelled' || b.cancelReason === 'spot_unavailable'),
    ),
)

// One comparison against the server-folded end instant, in place of walking every
// booking's date map. `Date.parse` rather than a string compare: the server emits
// RFC 3339 with an offset, which doesn't sort against an ISO "Z" string.
const stillToCome = (booking: RenterBookingResponse) => Date.parse(booking.endsAt) > Date.now()

const upcoming = computed(() => live.value.filter(stillToCome))
const past = computed(() => live.value.filter((b) => !stillToCome(b)))

/** After a cancel, re-read past the projection rather than trusting the cache. */
const refresh = () => queryClient.invalidateQueries({ queryKey: viewKeys.bookings })
</script>

<template>
    <Tabs default-value="upcoming" class="gap-5">
        <TabsList class="w-full group-data-horizontal/tabs:h-10">
            <TabsTrigger value="upcoming" class="font-bold">
                Upcoming
            </TabsTrigger>
            <TabsTrigger value="past" class="font-bold">
                Past
            </TabsTrigger>
        </TabsList>
        <TabsContent value="upcoming" class="space-y-2">
            <BookedSpotRow v-for="booking in upcoming" :key="booking?.id" :booking="booking"
                @changed="refresh" />
            <Text v-if="!upcoming.length" size="sm" class="py-8 text-center">
                Nothing booked yet.
            </Text>
        </TabsContent>
        <TabsContent value="past" class="space-y-2">
            <BookedSpotRow v-for="booking in past" :key="booking?.id" :booking="booking" past />
            <Text v-if="!past.length" size="sm" class="py-8 text-center">
                No past bookings.
            </Text>
        </TabsContent>
    </Tabs>
</template>
