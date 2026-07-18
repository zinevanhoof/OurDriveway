<script setup lang="ts">
import { BellIcon } from '@lucide/vue';
import { useRouter } from 'vue-router'

const router = useRouter()
</script>

<template>
    <BellIcon @click="() => router.push('/notifications')" class="size-6" />
</template>