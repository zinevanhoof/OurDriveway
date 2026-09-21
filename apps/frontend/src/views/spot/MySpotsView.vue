<script setup lang="ts">
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { Money } from '@/components/base/money';
import { SectionHeader } from '@/components/base/section-header';

import { ChevronRight, MapPin, Plus, TrendingDown, TrendingUp } from '@lucide/vue';
import Button from '@/components/ui/button/Button.vue';
import { Badge } from '@/components/ui/badge';
import { useRouter } from 'vue-router';
import { fetchHostSpots, fetchHostSummary } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/vue-query';
import { computed, onMounted } from 'vue';
import { Sentinel } from '@/components/base/sentinel';
import { isPlaceholder, placeholders, withLoadingRow } from '@/lib/placeholders';

const router = useRouter()
const queryClient = useQueryClient()

// No host variable: `GET /api/view/host/spots` takes the host from the token.
// Deleted listings are already excluded server-side, which the `deleted: { eq: false }`
// in the document this replaces had to say explicitly — a host's own *inactive*
// spots are still returned, because that is what the live switch is for.
//
// Paged, twenty at a time, like every list. The server says which offset is next.
const { data, fetchNextPage, hasNextPage, isFetchingNextPage } = useInfiniteQuery({
    queryKey: viewKeys.hostSpots,
    queryFn: ({ pageParam }) => fetchHostSpots({ offset: pageParam }),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
    placeholderData: placeholders.hostSpots,
})

// While the next page loads, one placeholder row at the bottom stands in for it.
const spots = computed(() =>
    withLoadingRow(
        data.value?.pages.flatMap((p) => p.spots) ?? [],
        isFetchingNextPage.value,
        placeholders.hostSpots.pages[0].spots,
    ),
)

// The two tiles. The same totals the profile reads, so the two screens share one cache.
const { data: summary, isPlaceholderData: summaryLoading } = useQuery({
    queryKey: viewKeys.hostSummary,
    queryFn: fetchHostSummary,
    placeholderData: placeholders.hostSummary,
})

/**
 * This month against last, as a whole percentage, or null when last month earned nothing
 * — a change from zero has no percentage worth printing.
 */
const monthChange = computed(() => {
    const s = summary.value
    if (!s || s.earnedLastMonthCents === 0) return null
    return Math.round(((s.earnedThisMonthCents - s.earnedLastMonthCents) / s.earnedLastMonthCents) * 100)
})

// A spot created in AddSpotView would otherwise be served from cache on arrival here.
// The write's `X-Version` already made the request wait for the projection; this is only about the
// client's own cache being fresher than `staleTime`.
onMounted(() => {
    if (history.state.refreshSpots) {
        void queryClient.invalidateQueries({ queryKey: viewKeys.spots })
    }
})
</script>

<template>
    <!-- Sized to the screen rather than to its content, down every level, so only the
         listing list scrolls. `min-h-0` on each flex child is what lets it shrink below
         its content; without it the chain grows and the page scrolls.

         No bottom padding here: the list scrolls, so its `pb-3` is inside its own
         scroll box, and that runs flush to the navbar. -->
    <div class="flex min-h-0 flex-1 flex-col gap-4 px-4 pt-4">
        <SectionHeader class="items-center">
            <Title size="xl" weight="extrabold">My parking spots</Title>
            <template #action>
                <Button @click="router.push({ name: 'spot-add' })" class="font-bold">
                    <Plus />
                    Add
                </Button>
            </template>
        </SectionHeader>
        <div class="grid grid-cols-2 gap-2">
            <Surface variant="elevated" size="lg">
                <Text weight="normal">Earned this month</Text>
                <Money :cents="summary?.earnedThisMonthCents ?? 0" size="2xl" :data-loading="summaryLoading" />
                <Text v-if="monthChange !== null" weight="normal" :tone="monthChange >= 0 ? 'success' : 'destructive'"
                    class="flex items-center gap-1" :data-loading="summaryLoading">
                    <component :is="monthChange >= 0 ? TrendingUp : TrendingDown" :size="14" />
                    {{ monthChange >= 0 ? '+' : '' }}{{ monthChange }}% vs last
                </Text>
            </Surface>
            <Surface variant="elevated" size="lg">
                <Text weight="normal">Active parking spots</Text>
                <Title size="2xl" :data-loading="summaryLoading">
                    {{ summary?.activeSpots ?? 0 }}/{{ summary?.spots ?? 0 }}
                </Title>
                <Text :data-loading="summaryLoading">
                    {{ summary?.bookedNow ? `${summary.bookedNow} booked right now` : 'None booked right now' }}
                </Text>
            </Surface>
        </div>
        <Text weight="semibold">All listings</Text>
        <!-- The only part that scrolls; everything above stays put. -->
        <div class="min-h-0 flex-1 overflow-y-auto no-scrollbar pb-3">
            <ul class="space-y-2">
                <Surface v-for="spot in spots" :key="spot.id" as="li" orientation="horizontal" class="gap-3"
                    :data-loading="isPlaceholder(spot.id)"
                    @click="router.push({ name: 'spot', params: { id: spot.id } })">
                    <img :src="spot.images[0]" class="size-12 rounded-sm object-cover">
                    <div class="flex-1 space-y-1">
                        <Title size="sm" weight="bold" class="flex items-center gap-2">
                            {{ spot.title }}
                            <!-- A paused listing looks identical to a live one otherwise,
                             and "why am I getting no bookings" is the question that
                             follows. -->
                            <Badge v-if="!spot.active" variant="secondary">Paused</Badge>
                        </Title>
                        <Text as="p" class="flex items-center gap-1 leading-normal">
                            <MapPin :size="14" />
                            {{ spot.address.line1 }} - {{ spot.address.city }}
                        </Text>
                    </div>
                    <div class="flex flex-col items-end">
                        <Money :cents="spot.pricePerHour" suffix="/hr" size="lg" weight="semibold" />
                        <ChevronRight class="text-muted-foreground" />
                    </div>
                </Surface>
            </ul>
            <!-- Crossing this asks for the next page. -->
            <Sentinel :has-next-page="hasNextPage" :fetching="isFetchingNextPage" @load="fetchNextPage" />
        </div>
    </div>
</template>