<script setup lang="ts">
import { cn } from '@/lib/utils'
import { X } from '@lucide/vue'

// Removable chips; click the label to make it active (edits its slots), X to remove.
defineProps<{ items: { key: string; label: string }[]; active: string }>()
defineEmits<{ select: [key: string]; remove: [key: string] }>()
</script>

<template>
    <div v-if="items.length" class="no-scrollbar flex gap-2 overflow-x-auto">
        <div v-for="it in items" :key="it.key"
            :class="cn('flex shrink-0 items-center gap-1 rounded-md py-1 pl-2.5 pr-1 text-sm font-semibold', it.key === active ? 'bg-primary text-primary-foreground' : 'bg-accent text-accent-foreground')">
            <button type="button" @click="$emit('select', it.key)" class="cursor-pointer">
                {{ it.label }}
            </button>
            <button type="button" @click="$emit('remove', it.key)" class="cursor-pointer opacity-70 hover:opacity-100">
                <X :size="14" />
            </button>
        </div>
    </div>
</template>
