<script setup lang="ts">
import { useHeader } from '@/composables/useHeader';
import mitt from 'mitt'

const bus = mitt();
const { title, actions } = useHeader();
</script>

<template>
    <header class="flex shrink-0 px-4 py-2 justify-between items-center border-b">
        <div class="text-xl">{{ title }}</div>
        <div class="flex items-center gap-4">
            <component v-for="(action, i) in actions" :key="i" :is="action.component" v-bind="action.props"
                @click="() => bus.emit('open-add-spot')" />
        </div>
    </header>
</template>