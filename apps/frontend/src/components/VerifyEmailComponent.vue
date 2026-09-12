<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'

import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { useVerifyEmail, useResendVerification } from '@/api/userApi'
import type { ApiError } from '@/api/client'

const route = useRoute()
const router = useRouter()

type State = 'verifying' | 'verified' | 'failed'
const state = ref<State>('verifying')
const error = ref<string>('')

// Prefilled from nothing — the token carries the user id, not their address, so
// a failed verification cannot tell us who to re-send to and has to ask.
const email = ref<string>('')

const { mutateAsync: verify } = useVerifyEmail()
const { mutateAsync: resendLink, isPending: resending } = useResendVerification()

/**
 * Runs on mount rather than behind a button.
 *
 * The link in the email is a plain GET to this page, and this POST is the only
 * thing with an effect. Mail scanners prefetch links but do not run the app, so
 * a prefetch loads the page and verifies nothing. Even if one did, the worst
 * case is an address that provably received the mail being marked as such —
 * which is exactly the claim verification makes.
 *
 * Swap this to a confirm button if that ever stops being an acceptable trade.
 */
onMounted(async () => {
    const token = route.query.token
    if (typeof token !== 'string' || !token) {
        state.value = 'failed'
        error.value = 'This link is missing its token. Request a new one below.'
        return
    }

    try {
        await verify(token)
        state.value = 'verified'
    } catch (e) {
        state.value = 'failed'
        error.value = (e as ApiError).detail[0]
    }
})

const resend = async () => {
    await resendLink(email.value)
    // Always the same message: the backend refuses to say whether the address is
    // registered, and echoing a difference here would undo that.
    toast.success('If that address has an account, a new link is on its way.')
}
</script>

<template>
    <div class="mx-4 mt-22 pb-2">
        <Surface size="lg" class="gap-6">
            <div class="grid gap-1">
                <Title class="font-medium">
                    <template v-if="state === 'verifying'">Verifying your email</template>
                    <template v-else-if="state === 'verified'">Email verified</template>
                    <template v-else>Link didn't work</template>
                </Title>
                <Text size="sm" weight="normal">
                    <template v-if="state === 'verifying'">One moment.</template>
                    <template v-else-if="state === 'verified'">
                        Your address is confirmed. You can log in now.
                    </template>
                    <template v-else>{{ error }}</template>
                </Text>
            </div>

            <div v-if="state === 'verifying'" class="flex justify-center py-6">
                <Spinner />
            </div>

            <div v-else-if="state === 'failed'" class="space-y-2">
                <Input v-model="email" type="email" placeholder="example@gmail.com" autocomplete="email" />
            </div>

            <Button v-if="state === 'verified'" class="w-full" @click="router.push({ name: 'login' })">
                Go to login
            </Button>

            <Button v-else-if="state === 'failed'" class="w-full" :disabled="resending || !email" @click="resend">
                Send a new link
            </Button>
        </Surface>
    </div>
</template>
