<script setup lang="ts">
import Separator from '@/components/ui/separator/Separator.vue';
import Button from '@/components/ui/button/Button.vue';
import { Drawer, DrawerContent } from '@/components/ui/drawer';
import { useQuery } from '@urql/vue';
import { ME } from '@/api/graphql/user.ts';
import { useAuthStore } from '@/stores/auth.ts';
import { computed, ref } from 'vue';
import { recordId } from '@/lib/utils.ts';
import Avatar from '@/components/ui/avatar/Avatar.vue';
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue';
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue';
import { Camera, ChevronRight, Lock, LogOut, UserRound } from '@lucide/vue';
import { formatCents } from '@/lib/money';
import { useRouter } from 'vue-router';
import { logoutUser } from '@/api/userApi';
import { imageUrl } from '@/lib/media'
import PayoutsComponent from '@/components/profile/PayoutsComponent.vue';

const auth = useAuthStore()
const router = useRouter()

const { data } = useQuery({
    query: ME,
    variables: computed(() => ({ id: recordId(auth.user?.id) })),
})

// No `!` here: the query has not resolved on first render, so this really is
// undefined for a tick and the template has to say so.
const me = computed(() => data.value?.user)

// Server first — revoking the refresh token needs the cookie, and clearing the
// store synchronously trips the `isAuthenticated` watcher in main.ts, which is
// what routes to login. No router.push here for that reason.
const confirmOpen = ref(false)

const logout = async () => {
    await logoutUser()
    auth.logout()
}
</script>

<template>
    <div class="px-4 space-y-2">
        <div v-if="me" class="p-4 border border-border rounded-md shadow-xs space-y-2 bg-card">
            <div class="flex items-center gap-3">
                <!-- Goes to the edit screen rather than opening a picker here. This
                     view is read-only — every other control on it is a row that
                     navigates — and a second upload path would need its own PATCH
                     and its own error handling for one shortcut. -->
                <button type="button" class="relative cursor-pointer"
                    @click="router.push({ name: 'profile-edit' })">
                    <Avatar size="3xl">
                        <AvatarImage v-if="me.profilePicture" :src="imageUrl(me.profilePicture)" />
                        <AvatarFallback :name="{ firstName: me.firstName, lastName: me.lastName }" />
                    </Avatar>
                    <span
                        class="absolute z-10 flex justify-center items-center right-0 bottom-0 bg-card rounded-full w-6 h-6 border border-border shadow-xs">
                        <Camera :size="16" class="text-primary" />
                    </span>
                    <span class="sr-only">Change profile picture</span>
                </button>
                <div>
                    <div class="text-lg font-bold">{{ me.firstName }} {{ me.lastName }}</div>
                    <div class="text-xs text-muted-foreground font-medium">{{ me.email }}</div>
                </div>
            </div>
            <Separator />
            <div class="flex text-center font-bold">
                <div class="flex-1">
                    3
                    <div class="text-xs text-muted-foreground">Spots</div>
                </div>
                <Separator orientation="vertical" />
                <div class="flex-1">
                    148
                    <div class="text-xs text-muted-foreground">Trips</div>
                </div>
                <Separator orientation="vertical" />
                <div class="flex-1">
                    {{ formatCents(124000) }}
                    <div class="text-xs text-muted-foreground">Earned</div>
                </div>
            </div>
        </div>
        <PayoutsComponent />

        <div class="bg-card border border-border rounded-md">
            <button type="button" class="flex gap-3 items-center p-3 w-full text-left"
                @click="router.push({ name: 'profile-edit' })">
                <div class="p-2 rounded-md bg-accent text-accent-foreground">
                    <UserRound :size="18" />
                </div>
                <div class="flex-1 font-semibold">Edit profile</div>
                <ChevronRight class="text-muted-foreground" />
            </button>
            <Separator />
            <button type="button" class="flex gap-3 items-center p-3 w-full text-left"
                @click="router.push({ name: 'profile-password' })">
                <div class="p-2 rounded-md bg-accent text-accent-foreground">
                    <Lock :size="18" />
                </div>
                <div class="flex-1 font-semibold">Change password</div>
                <ChevronRight class="text-muted-foreground" />
            </button>
        </div>
        <div class="bg-card border border-border rounded-md">
            <button type="button" class="flex gap-3 items-center p-3 w-full text-left" @click="confirmOpen = true">
                <div class="p-2 rounded-md bg-accent text-accent-foreground">
                    <LogOut :size="18" class="text-destructive" />
                </div>
                <div class="flex-1 font-semibold text-destructive">Log out</div>
                <ChevronRight class="text-muted-foreground" />
            </button>
        </div>

        <Drawer v-model:open="confirmOpen">
            <DrawerContent @close-auto-focus.prevent
                class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
                <div class="m-4 space-y-4">
                    <div>
                        <div class="text-lg font-bold">Log out?</div>
                        <div class="text-sm text-muted-foreground font-medium">
                            You'll need to sign in again to book or manage your spots.
                        </div>
                    </div>
                    <div class="space-y-2">
                        <Button variant="destructive" class="w-full h-11 font-bold" @click="logout">
                            Log out
                        </Button>
                        <Button variant="outline" class="w-full h-11 font-bold" @click="confirmOpen = false">
                            Cancel
                        </Button>
                    </div>
                </div>
            </DrawerContent>
        </Drawer>
    </div>
</template>