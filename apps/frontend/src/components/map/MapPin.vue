<script setup lang="ts">
import { formatCents } from '@/lib/money';
import { cn } from '@/lib/utils';
import Button from '../ui/button/Button.vue';

// One pin, two jobs: a single spot shows its hourly price, a cluster shows how many
// spots are stacked there. Deliberately not a price range — a cluster's members can
// cost anything, and one number out of several reads as *the* price.
const { pricePerHour = null, selected = false, count = 1 } = defineProps<{
    /** Null for a cluster, which has no single price to show. */
    pricePerHour?: number | null
    selected?: boolean
    /** How many spots this pin stands for. */
    count?: number
}>()

const emit = defineEmits(['select'])
</script>

<template>
    <Button size="sm" @click="emit('select')" :class="cn(
        'rounded-full font-bold border shadow-xl transition-transform duration-150',
        selected
            ? 'bg-primary text-primary-foreground border-card border-2 scale-125'
            : 'bg-card text-card-foreground border-border scale-100',
    )">
        {{ count > 1 ? `${count} spots` : formatCents(pricePerHour ?? 0) }}
    </Button>
</template>
