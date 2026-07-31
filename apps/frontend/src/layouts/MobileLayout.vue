<script setup lang="ts">
import MobileNavbar from '@/components/MobileNavbar.vue';
import { useAuthStore } from '@/stores/auth';
import { RouterView, useRoute } from 'vue-router';

const auth = useAuthStore();

const route = useRoute()
</script>

<template>
    <component :is="route.meta.header" v-if="auth.isAuthenticated && route.meta.header" />
    <main class="flex flex-1 flex-col overflow-y-auto bg-background text-foreground">
        <RouterView />
    </main>

    <MobileNavbar v-if="auth.isAuthenticated" />
</template>