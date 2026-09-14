<script setup lang="ts">
// Where a reset link lands. `ForgotPasswordComponent.vue` is what sends it.
//
// Unlike `VerifyEmailComponent`, nothing happens on mount: the effect needs a
// password the user types, so there is no prefetch to defend against and no reason
// to spend the token before they have decided what to set.
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod'
import { toast } from 'vue-sonner'

import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'

import { useResetPassword } from '@/api/userApi'
import type { ApiError } from '@/api/client'
import { passwordRules } from '@/lib/passwordSchema'

const route = useRoute()
const router = useRouter()

// Read once, not watched: the link is what opened this page, and a token arriving
// later would mean a navigation that does not happen.
const token = typeof route.query.token === 'string' ? route.query.token : ''

// The same shared chain signup and the change-password form use, because the
// backend holds all three to the same garde validators. No current password —
// the token is what proves the account is theirs.
const formSchema = toTypedSchema(
    z.object({
        password: passwordRules,
        confirmPassword: z.string(),
    }).refine((data) => data.password === data.confirmPassword, {
        message: 'Passwords do not match',
        path: ['confirmPassword'],
    })
)

const { handleSubmit, setErrors } = useForm({
    validationSchema: formSchema,
    initialValues: { password: '', confirmPassword: '' },
})

const formErrors = ref<string[]>([])

// A spent link, an expired one and a forged one all arrive as the same 400 — the
// backend refuses to tell them apart. So the failure state offers the one thing
// that helps in every case: ask for a new link.
const dead = ref(!token)

const { mutateAsync: reset, isPending: loading } = useResetPassword()

const submit = handleSubmit(async ({ password }) => {
    formErrors.value = []
    try {
        await reset({ token, password })

        // Every session on the account was just revoked server-side, this one
        // included — so there is nothing to go back to but the login screen.
        toast.success('Password changed. Log in with your new one.')
        router.push({ name: 'login' })
    } catch (e) {
        const err = e as ApiError
        if (err.applyTo(setErrors)) return
        // 422s land above, on the field. Anything else is the link itself.
        formErrors.value = err.detail
        dead.value = true
    }
})
</script>

<template>
    <div class="mx-4 mt-22 pb-2">
        <Surface size="lg" class="gap-6">
            <div class="grid gap-1">
                <Title class="font-medium">
                    {{ dead ? "Link didn't work" : 'Set a new password' }}
                </Title>
                <Text size="sm" weight="normal">
                    <template v-if="dead && formErrors.length">{{ formErrors[0] }}</template>
                    <template v-else-if="dead">
                        This link is missing its token. Request a new one below.
                    </template>
                    <template v-else>Pick something you haven't used here before.</template>
                </Text>
            </div>

            <form v-if="!dead" id="reset-password-form" @submit="submit">
                <FieldGroup class="gap-4">
                    <VeeField v-slot="{ componentField, errors }" name="password">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="reset-password-new">New password</FieldLabel>
                            <Input id="reset-password-new" type="password" v-bind="componentField"
                                placeholder="New password" autocomplete="new-password"
                                :aria-invalid="!!errors.length" />
                            <FieldDescription>
                                At least 8 characters, with an uppercase and a lowercase letter, a number and a
                                special character.
                            </FieldDescription>
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ componentField, errors }" name="confirmPassword">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="reset-password-confirm">Confirm new password</FieldLabel>
                            <Input id="reset-password-confirm" type="password" v-bind="componentField"
                                placeholder="New password" autocomplete="new-password"
                                :aria-invalid="!!errors.length" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <Button class="w-full" type="submit" :disabled="loading">
                        <Spinner v-if="loading" />
                        Set password
                    </Button>
                </FieldGroup>
            </form>

            <Button v-if="dead" class="w-full" @click="router.push({ name: 'forgot-password' })">
                Request a new link
            </Button>
            <Button v-else variant="outline" class="w-full" @click="router.push({ name: 'login' })">
                Back to login
            </Button>
        </Surface>
    </div>
</template>
