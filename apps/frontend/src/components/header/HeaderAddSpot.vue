<script setup lang="ts">
import { Plus } from '@lucide/vue';
import { useAddSpot } from '@/composables/useAddSpot'

const { trigger } = useAddSpot()
</script>

<template>
    <Plus v-motion :whilePress="{ scale: 0.9 }" class="size-6" @click="trigger" />
</template>