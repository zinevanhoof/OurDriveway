<script setup lang="ts">
import { ref, watch } from 'vue';
import { Search, SlidersHorizontal } from '@lucide/vue';
import { useDebounceFn } from '@vueuse/core';
import type { AcceptableValue } from 'reka-ui';
import {
    Combobox,
    ComboboxAnchor,
    ComboboxList,
    ComboboxViewport,
    ComboboxItem,
    ComboboxEmpty,
} from '@/components/ui/combobox';
import { suggestAddress } from '@/api/address';
import type { Address } from '@/types/domain/spot';
import type { SpotFilter } from '@/types/SpotFilter';
import MapSearchFilterComponent from './MapSearchFilterComponent.vue';
import Input from '../ui/input/Input.vue';
import Button from '../ui/button/Button.vue';
import { IconBox } from '@/components/base/icon-box';

const emit = defineEmits<{ select: [coords: [number, number]]; filter: [filter: SpotFilter] }>();

const term = ref('');
const items = ref<Address[]>([]);
const open = ref(false);
const filterOpen = ref(false);
const filterSummary = ref('');
// Set the input text on select without retriggering a search.
let suppress = false;

const search = useDebounceFn(async (q: string) => {
    const query = q.trim();
    if (query.length < 3) {
        items.value = [];
        open.value = false;
        return;
    }
    items.value = await suggestAddress(query);
    open.value = items.value.length > 0;
}, 300);

watch(term, (q) => {
    if (suppress) {
        suppress = false;
        return;
    }
    search(q);
});

const onSelect = (value: AcceptableValue) => {
    if (!value || typeof value !== 'object') return;
    const addr = value as Address;
    if (addr.lng == null || addr.lat == null) return;
    emit('select', [addr.lng, addr.lat]);
    suppress = true;
    term.value = addr.formatted;
    items.value = [];
    open.value = false;
};
</script>

<template>
    <div
        class="flex items-center gap-2 absolute bg-card text-card-foreground shadow-lg rounded-md p-2 top-2 left-2 right-2 z-1">
        <Combobox v-model:open="open" :ignore-filter="true" :reset-search-term-on-blur="false"
            @update:model-value="onSelect" class="flex-1 relative">
            <ComboboxAnchor>
                <Search
                    class="absolute left-2 top-1/2 -translate-y-1/2 size-6 text-muted-foreground pointer-events-none" />
                <Input v-model="term" placeholder="Search a place or address…"
                    class="pl-10 border-transparent focus-visible:border-transparent rounded-sm shadow-none font-semibold" />
            </ComboboxAnchor>
            <!-- List aligns to the anchor (the input), not the padded card. Widen by
                 the card's 1rem horizontal padding (stays centered → edges match the
                 card) and offset past its 0.5rem bottom padding. -->
            <ComboboxList :side-offset="12" class="w-[calc(var(--reka-combobox-trigger-width)+1rem)]">
                <ComboboxViewport>
                    <ComboboxEmpty>No matches</ComboboxEmpty>
                    <ComboboxItem v-for="(item, i) in items" :key="i" :value="item">
                        {{ item.formatted }}
                    </ComboboxItem>
                </ComboboxViewport>
            </ComboboxList>
        </Combobox>
        <MapSearchFilterComponent v-model:open="filterOpen" @update="filterSummary = $event"
            @apply="emit('filter', $event)">
            <IconBox size="lg" class="cursor-pointer">
                <SlidersHorizontal />
            </IconBox>
        </MapSearchFilterComponent>
        <Button v-if="filterSummary" size="xs" @click="filterOpen = true"
            class="absolute -bottom-4 left-2 rounded-full font-bold shadow-md">
            <SlidersHorizontal />
            {{ filterSummary }}
        </Button>
    </div>
</template>
