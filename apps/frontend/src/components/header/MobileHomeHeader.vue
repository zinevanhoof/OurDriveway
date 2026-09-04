<script setup lang="ts">
import { computed } from 'vue';
import { useAuthStore } from '@/stores/auth';
import { Bell } from '@lucide/vue';
import Avatar from '../ui/avatar/Avatar.vue';
import AvatarImage from '../ui/avatar/AvatarImage.vue';
import AvatarFallback from '../ui/avatar/AvatarFallback.vue';
import { useRouter } from 'vue-router';

const router = useRouter()

// Already fetched once at boot by fetchMe(); no reason for a second round trip.
const user = computed(() => useAuthStore().user);

const greeting = computed(() => {
    const h = new Date().getHours();
    if (h < 12) return "Good morning";
    if (h < 18) return "Good afternoon";
    return "Good evening";
});
</script>

<template>
    <header class="flex shrink-0 px-4 py-2 justify-between items-center bg-background">
        <div>
            <div class="text-sm text-muted-foreground">{{ greeting }}</div>
            <div class="text-2xl font-extrabold">{{ user?.firstName }} {{ user?.lastName }}</div>
        </div>
        <div class="flex items-center gap-3">
            <div
                class="flex items-center justify-center border border-border w-10 h-10 rounded-full bg-card text-muted-foreground">
                <bell :size="20" />
            </div>
            <Avatar @click="router.push({ name: 'profile' })" size="lg">
                <AvatarImage v-if="user?.profilePicture" :src="user.profilePicture" />
                <AvatarFallback :name="{ firstName: user?.firstName ?? '', lastName: user?.lastName ?? '' }" />
            </Avatar>
        </div>
    </header>
</template>