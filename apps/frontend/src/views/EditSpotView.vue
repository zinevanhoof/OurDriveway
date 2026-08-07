<script setup lang="ts">
import EditSpotComponent from '@/components/spot/EditSpotComponent.vue';


const { id } = defineProps<{ id: string }>()
</script>

<template>
    <EditSpotComponent :id="id" />
</template>