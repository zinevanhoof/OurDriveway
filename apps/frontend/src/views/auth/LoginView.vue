<script setup lang="ts">
import { toTypedSchema } from '@vee-validate/zod'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'

import {
    Field,
    FieldError,
    FieldGroup,
    FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Text, Title } from '@/components/base/text'
import { useLogin, useResendVerification } from '@/api/userApi'
import { fetchMe } from '@/api/me'
import { useAuthStore } from '@/stores/auth'
import type { ApiError } from '@/api/client'

const auth = useAuthStore()
const router = useRouter()

const formSchema = toTypedSchema(
    z.object({
        email: z.string().email(),
        password: z.string()
    })
)

const { handleSubmit, setErrors, isSubmitting, values } = useForm({
    validationSchema: formSchema,
    initialValues: {
        email: '',
        password: ''
    },
})

const serverErrors = ref<string[]>([])

// A 403 means the password was right but the address was never confirmed. That
// is a different conversation from "wrong credentials" — the fix is a link in
// their inbox, not another guess — so it gets its own state and a resend button
// instead of a red message under the form.
const unverified = ref(false)

const { mutateAsync: login } = useLogin()
const { mutateAsync: resendLink, isPending: resending } = useResendVerification()

const onSubmit = handleSubmit(async (data) => {
    serverErrors.value = []
    unverified.value = false

    try {
        const { accessToken } = await login(data)
        auth.setAccessToken(accessToken)
        auth.setUser(await fetchMe())
        router.push({ name: 'home' })
    } catch (e) {
        const err = e as ApiError

        if (err.isForbidden) {
            unverified.value = true
            return
        }

        // 422 puts a message under the offending input. Everything else — a 401
        // for a wrong password above all — is form-level, and `detail` carries
        // the backend's own words: "Invalid credentials".
        if (!err.applyTo(setErrors)) serverErrors.value = err.detail
    }
})

const resend = async () => {
    await resendLink(values.email ?? '')
    toast.success('New link sent. Check your inbox.')
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
            <Text as="p" size="md" weight="semibold" class="text-center">Welcome back</Text>
        </div>

        <form id="form-login" class="flex flex-col gap-4.5" @submit="onSubmit">
            <!-- The group holds the fields and nothing else. The link, the
                 form-level error and the submit button are siblings of it, not
                 members — they are not inputs and do not want its spacing. -->
            <FieldGroup class="gap-4.5">
                <VeeField v-slot="{ field, errors }" name="email">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="form-login-email">
                            Email
                        </FieldLabel>
                        <Input id="form-login-email" type="email" v-bind="field" placeholder="you@example.com"
                            autocomplete="email" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <VeeField v-slot="{ field, errors }" name="password">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="form-login-password">
                            Password
                        </FieldLabel>
                        <Input id="form-login-password" type="password" v-bind="field" placeholder="••••••••"
                            autocomplete="current-password" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
            </FieldGroup>

            <!-- An anchor, not a `Button`: it navigates, so it should be
                 right-clickable and openable in a new tab. `RouterLink` is
                 registered globally, so there is nothing to import. -->
            <RouterLink to="/forgot-password" class="self-end text-sm font-bold text-primary">
                Forgot password?
            </RouterLink>

            <FieldError v-if="serverErrors.length" :errors="serverErrors" />

            <Button type="submit" :disabled="isSubmitting" class="h-13 w-full rounded-lg text-[17px] font-extrabold">
                Log in
            </Button>
        </form>

        <template v-if="unverified">
            <Text as="p" size="sm" weight="normal" class="text-center">
                Verify your email address before logging in. Check your inbox.
            </Text>
            <Button variant="outline" :disabled="resending" class="h-13 w-full rounded-lg text-[17px] font-extrabold"
                @click="resend">
                Send the link again
            </Button>
        </template>

        <p class="mt-auto text-center text-[15px] font-semibold text-muted-foreground">
            New here?
            <RouterLink to="/signup" class="font-extrabold text-primary">Create an account</RouterLink>
        </p>
    </div>
</template>
