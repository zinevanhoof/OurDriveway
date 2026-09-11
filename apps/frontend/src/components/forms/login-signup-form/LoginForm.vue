<script setup lang="ts">
import { toTypedSchema } from '@vee-validate/zod'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { ref } from 'vue'

import {
    Field,
    FieldError,
    FieldGroup,
    FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { useLogin, useResendVerification } from '@/api/userApi'
import type { ApiError } from '@/api/client'
import type { LoginResponse } from '@/types/responses/user/LoginResponse'
import { toast } from 'vue-sonner'

const emit = defineEmits<{
    success: [LoginResponse]
}>()

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
        emit('success', await login(data))
    } catch (e) {
        const err = e as ApiError

        if (err.isForbidden) {
            unverified.value = true
            return
        }

        // 422 puts a message under the offending input. Everything else — a 401
        // for a wrong password above all — is form-level, and `detail` now
        // carries the backend's own words: "Invalid credentials", where this
        // used to render "Something went wrong. Please try again." because the
        // old client had already drained the response body.
        if (!err.applyTo(setErrors)) serverErrors.value = err.detail
    }
})

const resend = async () => {
    await resendLink(values.email ?? '')
    toast.success('New link sent. Check your inbox.')
}
</script>

<template>
    <Surface size="lg" class="gap-6">
        <div class="space-y-1">
            <Title weight="medium">Login</Title>
            <Text size="sm">
                Login into your account here
            </Text>
        </div>
        <form id="form-login" @submit="onSubmit">
            <FieldGroup>
                <VeeField v-slot="{ field, errors }" name="email">
                    <Field :data-invalid="!!errors.length">
                        <FieldLabel for="form-login-email">
                            Email
                        </FieldLabel>
                        <Input id="form-login-email" v-bind="field" placeholder="example@gmail.com" autocomplete="off"
                            :aria-invalid="!!errors.length" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <VeeField v-slot="{ field, errors }" name="password">
                    <Field :data-invalid="!!errors.length">
                        <FieldLabel for="form-login-password">
                            Password
                        </FieldLabel>
                        <Input type="password" id="form-login-password" v-bind="field" placeholder="Password"
                            autocomplete="off" :aria-invalid="!!errors.length" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <FieldError v-if="serverErrors.length" :errors="serverErrors" />

                <Button class="w-full" type="submit" :disabled="isSubmitting">
                    Login
                </Button>
            </FieldGroup>
        </form>
        <template v-if="unverified">
            <Text as="p" size="sm" weight="normal" class="text-center">
                Verify your email address before logging in. Check your inbox.
            </Text>
            <Button class="w-full" variant="outline" :disabled="resending" @click="resend">
                Send the link again
            </Button>
        </template>
    </Surface>
</template>
