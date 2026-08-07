<script setup lang="ts">
import { BOOKINGS_RENTED } from '@/api/graphql/booking';
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'
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
// full of — an abandoned checkout in particular is noise, not a record. The one
// exception is a booking the *host* withdrew: that one has to stay visible, or a
// trip someone paid for just disappears without ever saying why.
const live = computed(() =>
    (data.value?.bookings ?? []).filter(
        (b: any) =>
            b?.status !== 'released' &&
            (b?.status !== 'cancelled' || b?.cancelReason === 'spot_unavailable'),
    ),
)

// One comparison against the server-folded end instant, in place of walking every
// booking's date map. `Date.parse` rather than a string compare: the server emits
// RFC 3339 with an offset, which doesn't sort against an ISO "Z" string.
const stillToCome = (booking: any) => Date.parse(booking?.endsAt) > Date.now()

const upcoming = computed(() => live.value.filter(stillToCome))
const past = computed(() => live.value.filter((b: any) => !stillToCome(b)))

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
