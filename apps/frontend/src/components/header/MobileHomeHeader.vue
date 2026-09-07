<script setup lang="ts">
import { computed } from 'vue';
import { useAuthStore } from '@/stores/auth';
import { Bell } from '@lucide/vue';
import Avatar from '../ui/avatar/Avatar.vue';
import AvatarImage from '../ui/avatar/AvatarImage.vue';
import AvatarFallback from '../ui/avatar/AvatarFallback.vue';
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { IconBox } from '@/components/base/icon-box';
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
            <Text size="sm" weight="normal">{{ greeting }}</Text>
            <Title size="2xl" weight="extrabold">{{ user?.firstName }} {{ user?.lastName }}</Title>
        </div>
        <Surface variant="none" size="none" orientation="horizontal" class="gap-3">
            <IconBox size="lg" tone="card" shape="circle">
                <Bell />
            </IconBox>
            <Avatar @click="router.push({ name: 'profile' })" size="lg">
                <AvatarImage v-if="user?.profilePicture" :src="user.profilePicture" />
                <AvatarFallback :name="{ firstName: user?.firstName ?? '', lastName: user?.lastName ?? '' }" />
            </Avatar>
        </Surface>
    </header>
</template>