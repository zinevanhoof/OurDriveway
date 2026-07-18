<script setup lang="ts">
import { Home, ParkingSquare, Search, User } from '@lucide/vue';
import { motion } from 'motion-v';
import { RouterLink, useRoute } from 'vue-router';

const route = useRoute();

const items = [
    { id: "home", path: "/", icon: Home },
    { id: "search", path: "/search", icon: Search },
    { id: "spots", path: "/spots", icon: ParkingSquare },
    { id: "profile", path: "/profile", icon: User }
]
</script>

<template>
    <nav class="flex shrink-0 h-10 justify-around items-center border-t">
        <RouterLink @click="route.path === item.path" v-for="item in items" :key="item.id" :to="item.path"
            class="relative flex items-center justify-center">
            <component :is="item.icon" />

            <motion.div v-if="route.path === item.path" layoutId="nav-indicator"
                class="absolute -bottom-2 h-1 w-8 rounded-full bg-primary" :transition="{
                    type: 'spring',
                    stiffness: 500,
                    damping: 35
                }" />
        </RouterLink>
    </nav>
</template>