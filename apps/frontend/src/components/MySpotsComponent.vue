<script setup lang="ts">
import { formatCents } from '@/lib/money';

import {
    Item,
    ItemContent,
    ItemTitle,
} from '@/components/ui/item'

import ItemDescription from '@/components/ui/item/ItemDescription.vue';
import { ChevronRight, MapPin, Plus, TrendingUp } from '@lucide/vue';
import Button from '@/components/ui/button/Button.vue';
import Card from '@/components/ui/card/Card.vue';
import CardContent from '@/components/ui/card/CardContent.vue';
import ItemMedia from '@/components/ui/item/ItemMedia.vue';
import ItemGroup from '@/components/ui/item/ItemGroup.vue';
import { useRouter } from 'vue-router';
import { useAuthStore } from '@/stores/auth';
import { SPOTS_OWNED } from '@/api/graphql/spot';
import { useQuery } from '@urql/vue';
import { computed, onMounted } from 'vue';
import { imageUrl } from '@/lib/media'

const router = useRouter()

const auth = useAuthStore()

// Declarative subscription: fetches on mount, exposes reactive `data`.
// Must be set up here in setup(), not inside an event handler.
const { data, executeQuery } = useQuery({
    query: SPOTS_OWNED,
    variables: computed(() => ({ id: auth.user?.id })),
})

// urql serves the cached list on mount, so a spot just created in AddSpotView
// wouldn't show up without forcing a network fetch.
onMounted(() => {
    if (history.state.refreshSpots) executeQuery({ requestPolicy: 'network-only' })
})
</script>

<template>
    <div class="space-y-4">
        <div class="flex justify-between items-center">
            <div class="text-xl font-extrabold text-foreground">Your parking spots</div>
            <Button @click="router.push({ name: 'spot-add' })" class="font-bold">
                <Plus />
                Add
            </Button>
        </div>
        <div class="flex gap-2">
            <Card size="sm" class="flex-1">
                <CardContent>
                    <div class="text-xs text-muted-foreground">Earned this month</div>
                    <div class="text-2xl font-bold">$266</div>
                    <div class="flex items-center gap-1 text-success text-xs">
                        <TrendingUp :size="14" />
                        +18% vs last
                    </div>
                </CardContent>
            </Card>
            <Card size="sm" class="flex-1">
                <CardContent>
                    <div class="text-xs text-muted-foreground">Active parking spots</div>
                    <div class="text-2xl font-bold">2/3</div>
                    <div class="text-xs text-muted-foreground font-medium">1 booked right now</div>
                </CardContent>
            </Card>
        </div>
        <div class="text-xs text-muted-foreground font-semibold">All listings</div>
        <ItemGroup class="cursor-pointer gap-2">
            <Item @click="() => router.push({ name: 'spot', params: { id: spot.id } })" v-for="spot in data?.spots"
                :key="spot.id" variant="outline" class="bg-card">
                <ItemMedia variant="image"
                    class="group-has-data-[slot=item-description]/item:self-center group-has-data-[slot=item-description]/item:translate-y-0">
                    <img :src="imageUrl(spot.images[0])">
                </ItemMedia>
                <ItemContent>
                    <ItemTitle class="font-bold">
                        {{ spot.title }}
                        <!-- A paused listing looks identical to a live one otherwise,
                             and "why am I getting no bookings" is the question that
                             follows. -->
                        <span v-if="!spot.active"
                            class="rounded-full bg-muted px-2 py-0.5 text-xs font-semibold text-muted-foreground">
                            Paused
                        </span>
                    </ItemTitle>
                    <ItemDescription class="flex items-center gap-1 text-muted-foreground text-xs font-medium">
                        <MapPin :size="14" />
                        {{ spot.address.line1 }} - {{ spot.address.city }}
                    </ItemDescription>
                </ItemContent>
                <ItemContent class="items-end">
                    <div class="flex items-baseline text-lg font-semibold">{{
                        formatCents(Number(spot.price_per_hour)) }}
                        <div class="text-xs text-muted-foreground font-medium">/hr</div>
                    </div>
                    <ChevronRight class="text-muted-foreground" />
                </ItemContent>
            </Item>
        </ItemGroup>
    </div>
</template>