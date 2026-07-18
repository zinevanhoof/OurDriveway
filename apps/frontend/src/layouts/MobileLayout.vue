<script setup lang="ts">
import MobileHeader from '@/components/header/MobileHeader.vue';
import MobileNavbar from '@/components/MobileNavbar.vue';
import { useAuthStore } from '@/stores/auth';
import { RouterView } from 'vue-router';

const auth = useAuthStore();
</script>

<template>
    <MobileHeader v-if="auth.isAuthenticated" />
    <main class="flex flex-1 flex-col overflow-y-auto">
        <RouterView />
    </main>

    <MobileNavbar v-if="auth.isAuthenticated" />
</template>