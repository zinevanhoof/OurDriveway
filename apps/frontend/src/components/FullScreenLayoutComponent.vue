<script setup lang="ts">
import { X } from '@lucide/vue';
import Button from './ui/button/Button.vue';

const { title, description } = defineProps<{
    title: string
    description?: string
}>()

const emit = defineEmits(['close'])
</script>

<template>
    <header class="flex items-center gap-2 px-4 py-3.5 bg-card border-b border-border">
        <Button size="icon" @click="emit('close')" class="bg-muted text-foreground">
            <X />
        </Button>
        <div>
            <div class="text-[16px] font-extrabold">{{ title }}</div>
            <div v-if="description" class="text-xs text-muted-foreground font-semibold">{{ description }}</div>
        </div>
    </header>
    <main class="space-y-4 flex-1 px-4 pb-4 pt-4 bg-background overflow-y-auto no-scrollbar">
        <slot name="main"></slot>
    </main>
    <footer class="px-4 pb-4 pt-3 bg-card border-t border-border">
        <slot name="footer"></slot>
    </footer>
</template>