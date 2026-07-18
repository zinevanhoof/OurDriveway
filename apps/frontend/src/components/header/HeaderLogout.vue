<script setup lang="ts">
import { logoutUser } from '@/api/userApi';
import { useRouter } from 'vue-router';
import { useAuthStore } from '@/stores/auth';
import { LogOutIcon } from '@lucide/vue';

import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle,
    AlertDialogTrigger,
} from '@/components/ui/alert-dialog'

const auth = useAuthStore()
const router = useRouter()

const logout = async () => {
    await logoutUser()
    auth.logout()
    router.push({ name: 'login' })
}
</script>

<template>
    <AlertDialog>
        <AlertDialogTrigger>
            <LogOutIcon class="size-6 text-red-400" />
        </AlertDialogTrigger>
        <AlertDialogContent>
            <AlertDialogHeader>
                <AlertDialogTitle>Are you absolutely sure?</AlertDialogTitle>
                <AlertDialogDescription>
                    This will log you out of your account.
                </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
                <AlertDialogCancel variant="destructive">Cancel</AlertDialogCancel>
                <AlertDialogAction @click="logout">Logout</AlertDialogAction>
            </AlertDialogFooter>
        </AlertDialogContent>
    </AlertDialog>
</template>