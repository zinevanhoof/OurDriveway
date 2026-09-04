<script setup lang="ts">
import { cn } from '@/lib/utils';
import { Home, ParkingSquare, Search, Wallet } from '@lucide/vue';
import { motion } from 'motion-v';
import { RouterLink, useRoute } from 'vue-router';

const route = useRoute();

const items = [
    { id: "Home", path: "/", icon: Home },
    { id: "Search", path: "/search", icon: Search },
    { id: "Spots", path: "/spots", icon: ParkingSquare },
    { id: "Wallet", path: "/wallet", icon: Wallet }
]
</script>

<template>
    <nav
        class="pointer-events-auto relative z-60 flex h-15 justify-around items-center border-t bg-card text-muted-foreground">
        <!-- Opaque backing over the safe-area strip below the bar, so drawers/overlays
             don't show through the translucent Android navigation bar. -->
        <div class="absolute inset-x-0 top-full h-(--safe-bottom) bg-card" />
        <RouterLink v-for="item in items" :key="item.id" :to="item.path"
            :class="cn('relative flex flex-col items-center justify-center gap-1 h-full transition-all', route.path === item.path ? 'text-primary' : '')">
            <motion.div v-if="route.path === item.path" layoutId="nav-indicator"
                class="absolute top-0 h-0.75 w-6 rounded-full bg-primary" :transition="{
                    type: 'spring',
                    stiffness: 500,
                    damping: 35
                }" />
            <component :is="item.icon" />
            <div class="text-xs font-semibold">{{ item.id }}</div>
        </RouterLink>
    </nav>
</template>