<script setup lang="ts">
import SpotBookingsComponent from "@/components/spot/SpotBookingsComponent.vue";

const { id } = defineProps<{ id: string }>()
</script>

<template>
    <SpotBookingsComponent :id="id" />
</template>
