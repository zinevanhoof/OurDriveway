<script setup lang="ts">
// Asks for a reset link. The other half of the flow is
// `ResetPasswordComponent.vue`, which is where the link lands.
//
// The `Surface` shell rather than `FullScreenLayoutComponent`, matching
// `VerifyEmailComponent`: this is reachable by someone with no session, and that
// layout's close button is `router.back()`, which from a fresh tab goes nowhere.
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod'

import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { Field, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'

import { useForgotPassword } from '@/api/userApi'
import type { ApiError } from '@/api/client'

const router = useRouter()

const formSchema = toTypedSchema(
    z.object({ email: z.string().email() })
)

const { handleSubmit, setErrors } = useForm({
    validationSchema: formSchema,
    initialValues: { email: '' },
})

const formErrors = ref<string[]>([])

// Not just "hide the form": each send invalidates the link the previous one
// mailed, so a second submit turns the first email into a dead end for anyone who
// opens it after the second. Removing the button is the cheapest guard against
// the impatient double-tap that causes it.
const sent = ref(false)

const { mutateAsync: requestLink, isPending: loading } = useForgotPassword()

const submit = handleSubmit(async ({ email }) => {
    formErrors.value = []
    try {
        await requestLink(email)
        sent.value = true
    } catch (e) {
        const err = e as ApiError
        if (!err.applyTo(setErrors)) formErrors.value = err.detail
    }
})
</script>

<template>
    <div class="mx-4 mt-22 pb-2">
        <Surface size="lg" class="gap-6">
            <div class="grid gap-1">
                <Title class="font-medium">
                    {{ sent ? 'Check your inbox' : 'Forgot password' }}
                </Title>
                <Text size="sm" weight="normal">
                    <!-- Says nothing about whether the address exists, because the
                         backend deliberately answers the same either way. -->
                    <template v-if="sent">
                        If that address has an account, a reset link is on its way. It expires in an
                        hour, and only the most recent link works.
                    </template>
                    <template v-else>
                        Enter your email and we'll send you a link to set a new password.
                    </template>
                </Text>
            </div>

            <form v-if="!sent" id="forgot-password-form" @submit="submit">
                <FieldGroup class="gap-4">
                    <VeeField v-slot="{ componentField, errors }" name="email">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="forgot-password-email">Email</FieldLabel>
                            <Input id="forgot-password-email" type="email" v-bind="componentField"
                                placeholder="example@gmail.com" autocomplete="email"
                                :aria-invalid="!!errors.length" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <FieldError v-if="formErrors.length" :errors="formErrors" />

                    <Button class="w-full" type="submit" :disabled="loading">
                        <Spinner v-if="loading" />
                        Send reset link
                    </Button>
                </FieldGroup>
            </form>

            <Button variant="outline" class="w-full" @click="router.push({ name: 'login' })">
                Back to login
            </Button>
        </Surface>
    </div>
</template>
