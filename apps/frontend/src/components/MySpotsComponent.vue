<script setup lang="ts">
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { Money } from '@/components/base/money';
import { SectionHeader } from '@/components/base/section-header';

import { ChevronRight, MapPin, Plus, TrendingUp } from '@lucide/vue';
import Button from '@/components/ui/button/Button.vue';
import { Badge } from '@/components/ui/badge';
import { useRouter } from 'vue-router';
import { fetchHostSpots, viewKeys } from '@/api/viewApi';
import { useQuery, useQueryClient } from '@tanstack/vue-query';
import { computed, onMounted } from 'vue';

const router = useRouter()
const queryClient = useQueryClient()

// No host variable: `GET /api/view/host/spots` takes the host from the token.
// Deleted listings are already excluded server-side, which the `deleted: { eq: false }`
// in the document this replaces had to say explicitly — a host's own *inactive*
// spots are still returned, because that is what the live switch is for.
const { data } = useQuery({
    queryKey: viewKeys.hostSpots,
    queryFn: fetchHostSpots,
})

const spots = computed(() => data.value ?? [])

// A spot created in AddSpotView would otherwise be served from cache on arrival here.
// `recordSeq` already made the request wait for the projection; this is only about the
// client's own cache being fresher than `staleTime`.
onMounted(() => {
    if (history.state.refreshSpots) {
        void queryClient.invalidateQueries({ queryKey: viewKeys.spots })
    }
})
</script>

<template>
    <div class="space-y-4">
        <SectionHeader class="items-center">
            <Title size="xl" weight="extrabold">Your parking spots</Title>
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
                <Title size="2xl">$266</Title>
                <Text weight="normal" tone="success" class="flex items-center gap-1">
                    <TrendingUp :size="14" />
                    +18% vs last
                </Text>
            </Surface>
            <Surface variant="elevated" size="lg">
                <Text weight="normal">Active parking spots</Text>
                <Title size="2xl">2/3</Title>
                <Text>1 booked right now</Text>
            </Surface>
        </div>
        <Text weight="semibold">All listings</Text>
        <ul class="space-y-2">
            <Surface v-for="spot in spots" :key="spot.id" as="li" orientation="horizontal" class="gap-3"
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
    </div>
</template>