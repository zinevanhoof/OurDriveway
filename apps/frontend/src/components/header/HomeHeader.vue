<script setup lang="ts">
import { computed } from 'vue';
import { useQuery } from '@tanstack/vue-query';
import { fetchNotifications } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
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

// Already fetched once at boot by fetchAccount(); no reason for a second round trip.
const user = computed(() => useAuthStore().user);

// Polled: a rating prompt comes due when a booking ends, and a minute late is fine.
const { data: notifications } = useQuery({
    queryKey: viewKeys.notifications,
    queryFn: fetchNotifications,
    refetchInterval: 60_000,
});
const unseen = computed(() => notifications.value?.filter((n) => !n.seen).length ?? 0);

const greeting = computed(() => {
    const h = new Date().getHours();
    if (h < 12) return "Good morning";
    if (h < 18) return "Good afternoon";
    return "Good evening";
});
</script>

<template>
    <header class="flex shrink-0 px-4 py-2 justify-between items-center">
        <div>
            <Text size="sm" weight="normal">{{ greeting }}</Text>
            <Title size="2xl" weight="extrabold">{{ user?.firstName }} {{ user?.lastName }}</Title>
        </div>
        <Surface variant="none" size="none" orientation="horizontal" class="gap-3">
            <button type="button" class="relative" :aria-label="`Notifications, ${unseen} new`"
                @click="router.push({ name: 'notifications' })">
                <IconBox size="lg" tone="card" shape="circle">
                    <Bell />
                </IconBox>
                <span v-if="unseen"
                    class="absolute -right-1 -top-1 flex min-w-5 h-5 items-center justify-center rounded-full bg-destructive px-1 text-xs font-semibold text-white">
                    {{ unseen > 9 ? '9+' : unseen }}
                </span>
            </button>
            <Avatar @click="router.push({ name: 'profile' })" size="lg">
                <AvatarImage v-if="user?.profilePicture" :src="user.profilePicture" />
                <AvatarFallback :name="{ firstName: user?.firstName ?? '', lastName: user?.lastName ?? '' }" />
            </Avatar>
        </Surface>
    </header>
</template>