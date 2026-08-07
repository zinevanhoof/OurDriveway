<script setup lang="ts">
import ManageSpotComponent from "@/components/spot/ManageSpotComponent.vue";

const { id } = defineProps<{ id: string }>()
</script>

<template>
    <ManageSpotComponent :id="id" />
</template>