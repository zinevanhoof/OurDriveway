<script setup lang="ts">
import Separator from '@/components/ui/separator/Separator.vue';
import Button from '@/components/ui/button/Button.vue';
import { Drawer, DrawerContent } from '@/components/ui/drawer';
import { useQuery } from '@tanstack/vue-query';
import { fetchAccount } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { useAuthStore } from '@/stores/auth.ts';
import { computed, ref } from 'vue';
import Avatar from '@/components/ui/avatar/Avatar.vue';
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue';
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue';
import { Camera, ChevronRight, Lock, LogOut, UserRound } from '@lucide/vue';
import { formatCents } from '@/lib/money';
import { Surface } from '@/components/base/surface';
import { Text, Title } from '@/components/base/text';
import { IconBox } from '@/components/base/icon-box';
import { useRouter } from 'vue-router';
import { useLogout } from '@/api/userApi';

const auth = useAuthStore()
const router = useRouter()

// No id variable: the server picks the row from the verified claim. The `ME` document
// this replaces passed `gqlRecordId(auth.user?.id)`, which needed the `u'<uuid>'`
// spelling a record lookup takes — a distinction that no longer exists.
const { data } = useQuery({
    queryKey: viewKeys.account,
    queryFn: fetchAccount,
})

// No `!` here: the query has not resolved on first render, so this really is
// undefined for a tick and the template has to say so. `profile` is additionally null
// for the moment between registering and that projection landing.
const me = computed(() => data.value?.profile)

// Server first — revoking the refresh token needs the cookie, and clearing the
// store synchronously trips the `isAuthenticated` watcher in main.ts, which is
// what routes to login. No router.push here for that reason.
const confirmOpen = ref(false)

const { mutateAsync: endSession } = useLogout()

const logout = async () => {
    // Even a failed revoke ends the session here: the refresh cookie may already be
    // gone, and leaving someone signed in because logout 500'd is the wrong way to
    // fail. `useLogout` clears the query cache either way.
    await endSession().catch(() => {})
    auth.logout()
}
</script>

<template>
    <div class="px-4 pb-2 space-y-2">
        <Surface v-if="me" variant="elevated" size="lg" class="gap-2">
            <Surface variant="none" size="none" orientation="horizontal" class="gap-3">
                <!-- Goes to the edit screen rather than opening a picker here. This
                     view is read-only — every other control on it is a row that
                     navigates — and a second upload path would need its own PATCH
                     and its own error handling for one shortcut. -->
                <button type="button" class="relative cursor-pointer"
                    @click="router.push({ name: 'profile-edit' })">
                    <Avatar size="3xl">
                        <AvatarImage v-if="me.profilePicture" :src="me.profilePicture" />
                        <AvatarFallback :name="{ firstName: me.firstName, lastName: me.lastName }" />
                    </Avatar>
                    <span
                        class="absolute z-10 flex justify-center items-center right-0 bottom-0 bg-card rounded-full w-6 h-6 border border-border shadow-xs">
                        <Camera :size="16" class="text-primary" />
                    </span>
                    <span class="sr-only">Change profile picture</span>
                </button>
                <div>
                    <Title size="lg">{{ me.firstName }} {{ me.lastName }}</Title>
                    <Text>{{ me.email }}</Text>
                </div>
            </Surface>
            <Separator />
            <div class="flex text-center font-bold">
                <div class="flex-1">
                    3
                    <Text weight="bold">Spots</Text>
                </div>
                <Separator orientation="vertical" />
                <div class="flex-1">
                    148
                    <Text weight="bold">Trips</Text>
                </div>
                <Separator orientation="vertical" />
                <div class="flex-1">
                    {{ formatCents(124000) }}
                    <Text weight="bold">Earned</Text>
                </div>
            </div>
        </Surface>

        <Surface size="none">
            <Surface as="button" variant="none" orientation="horizontal" type="button" class="w-full gap-3 text-left"
                @click="router.push({ name: 'profile-edit' })">
                <IconBox>
                    <UserRound />
                </IconBox>
                <Title weight="semibold" class="flex-1">Edit profile</Title>
                <ChevronRight class="text-muted-foreground" />
            </Surface>
            <Separator />
            <Surface as="button" variant="none" orientation="horizontal" type="button" class="w-full gap-3 text-left"
                @click="router.push({ name: 'profile-password' })">
                <IconBox>
                    <Lock />
                </IconBox>
                <Title weight="semibold" class="flex-1">Change password</Title>
                <ChevronRight class="text-muted-foreground" />
            </Surface>
        </Surface>
        <Surface size="none">
            <Surface as="button" variant="none" orientation="horizontal" type="button" class="w-full gap-3 text-left"
                @click="confirmOpen = true">
                <IconBox>
                    <LogOut class="text-destructive" />
                </IconBox>
                <Title weight="semibold" tone="destructive" class="flex-1">Log out</Title>
                <ChevronRight class="text-muted-foreground" />
            </Surface>
        </Surface>

        <Drawer v-model:open="confirmOpen">
            <DrawerContent @close-auto-focus.prevent
                class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
                <div class="m-4 space-y-4">
                    <div>
                        <Title size="lg">Log out?</Title>
                        <Text size="sm">
                            You'll need to sign in again to book or manage your spots.
                        </Text>
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