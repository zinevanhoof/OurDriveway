<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'

import { Field, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { Text, Title } from '@/components/base/text'
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
    <div class="flex flex-1 flex-col gap-9 px-7 pt-24 pb-12">
        <div class="flex flex-col items-center gap-4">
            <img src="/ourdriveway-icon.svg" alt="OurDriveway" width="84" height="84"
                class="size-21 rounded-[20px] shadow-[0_16px_32px_-14px] shadow-primary/50" />
            <Title as="h1" class="text-3xl leading-none tracking-[-0.02em] whitespace-nowrap">
                <span class="font-bold text-primary">Our</span><span class="font-extrabold">Driveway</span>
            </Title>
            <Text as="p" size="md" weight="semibold" class="text-center text-balance">
                <template v-if="state === 'verifying'">Verifying your email. One moment.</template>
                <template v-else-if="state === 'verified'">
                    Your address is confirmed. You can log in now.
                </template>
                <template v-else>{{ error }}</template>
            </Text>
        </div>

        <div v-if="state === 'verifying'" class="flex justify-center py-6">
            <Spinner class="size-8" />
        </div>

        <Button v-else-if="state === 'verified'" class="h-13 w-full rounded-lg text-[17px] font-extrabold" @click="router.push({ name: 'login' })">
            Go to login
        </Button>

        <!-- A form, not a bare input and a button: this is the one field on the
             screen, and Enter should send it like it does on every other auth
             screen. No vee-validate — there is nothing to validate beyond "not
             empty", which the button's `disabled` already says. -->
        <form v-else class="flex flex-col gap-4.5" @submit.prevent="resend">
            <FieldGroup class="gap-4.5">
                <Field class="gap-2">
                    <FieldLabel class="font-bold" for="verify-email-resend">
                        Email
                    </FieldLabel>
                    <Input id="verify-email-resend" v-model="email" type="email" placeholder="you@example.com"
                        autocomplete="email" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                </Field>
            </FieldGroup>

            <Button type="submit" :disabled="resending || !email" class="h-13 w-full rounded-lg text-[17px] font-extrabold">
                <Spinner v-if="resending" />
                Send a new link
            </Button>
        </form>

        <!-- Left out once verified: "Go to login" above is already that button, and
             two of them is one too many. -->
        <p v-if="state !== 'verified'" class="mt-auto text-center text-[15px] font-semibold text-muted-foreground">
            Already verified?
            <RouterLink to="/login" class="font-extrabold text-primary">Log in</RouterLink>
        </p>
    </div>
</template>
