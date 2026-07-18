<script setup lang="ts">
import { SPOT } from '@/api/graphql/spot';
import { useServiceQuery } from '@/composables/useServiceQuery';
import { computed } from 'vue';

const { id } = defineProps<{ id: string }>()

const { data } = useServiceQuery("spot", {
    query: SPOT,
    variables: computed(() => ({ id: id.split(':')[1] })),
})
</script>

<template>
    <p class="mt-2 mx-2">{{ data?.spot?.title }}</p>
</template>