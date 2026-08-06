<script setup lang="ts">
import { BOOKINGS_RENTED } from '@/api/graphql/booking';
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'
import { isUpcoming } from '@/lib/bookingDates';
import { useAuthStore } from '@/stores/auth';
import { useQuery } from '@urql/vue';
import { computed } from 'vue';
import BookedSpotRow from './BookedSpotRow.vue';

const auth = useAuthStore()

const { data, executeQuery } = useQuery({
    query: BOOKINGS_RENTED,
    variables: computed(() => ({ renterId: auth.user?.id })),
    // Otherwise this fires once with renterId: undefined, before main.ts has
    // rehydrated the session.
    pause: computed(() => !auth.user?.id),
})

// A booking that ended without happening is not history the renter wants a tab
// full of — an abandoned checkout in particular is noise, not a record. Anything
// still live is bucketed purely on whether a day is left, in the spot's zone.
const live = computed(() =>
    (data.value?.bookings ?? []).filter(
        (b: any) => b?.status !== 'released' && b?.status !== 'cancelled',
    ),
)
const upcoming = computed(() => live.value.filter((b: any) => isUpcoming(b, b?.spot?.timezone)))
const past = computed(() => live.value.filter((b: any) => !isUpcoming(b, b?.spot?.timezone)))

/** After a cancel, re-read past the projection rather than trusting the cache. */
const refresh = () => executeQuery({ requestPolicy: 'network-only' })
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
            <div v-if="!upcoming.length" class="py-8 text-center text-sm text-muted-foreground font-medium">
                Nothing booked yet.
            </div>
        </TabsContent>
        <TabsContent value="past" class="space-y-2">
            <BookedSpotRow v-for="booking in past" :key="booking?.id" :booking="booking" past />
            <div v-if="!past.length" class="py-8 text-center text-sm text-muted-foreground font-medium">
                No past bookings.
            </div>
        </TabsContent>
    </Tabs>
</template>
