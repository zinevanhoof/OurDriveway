<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod'
import { toast } from 'vue-sonner'

import FullScreenLayoutComponent from '../FullScreenLayoutComponent.vue'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'

import { changePassword } from '@/api/userApi'
import { applyValidationErrors, readErrorDetail } from '@/lib/serverErrors'
import { passwordRules } from '@/lib/passwordSchema'

const router = useRouter()

// Same rules as signup for the new password — one shared chain, because the
// backend holds both to the same garde validators. The current password is only
// checked for presence: it is verified against the stored hash, and complexity
// rules on it would lock out anyone whose password predates the rules.
const formSchema = toTypedSchema(
    z.object({
        currentPassword: z.string().min(1, 'Enter your current password.'),
        newPassword: passwordRules,
        confirmPassword: z.string(),
    }).refine((data) => data.newPassword === data.confirmPassword, {
        message: 'Passwords do not match',
        path: ['confirmPassword'],
    })
)

const { handleSubmit, setErrors } = useForm({
    validationSchema: formSchema,
    initialValues: { currentPassword: '', newPassword: '', confirmPassword: '' },
})

const loading = ref(false)
const formErrors = ref<string[]>([])

const submit = handleSubmit(async ({ confirmPassword, ...form }) => {
    formErrors.value = []
    loading.value = true
    try {
        const response = await changePassword(form)

        if (!response.ok) {
            if (await applyValidationErrors(response, setErrors)) return
            formErrors.value = await readErrorDetail(response)
            return
        }

        // Sessions are deliberately left alone, so there is nothing to re-auth:
        // the current access and refresh tokens keep working.
        toast.success('Password changed')
        router.back()
    } finally {
        loading.value = false
    }
})
</script>

<template>
    <FullScreenLayoutComponent @close="router.back()" title="Change password"
        description="Confirm your current one to set a new one">
        <template #main>
            <form id="change-password-form" @submit="submit" class="space-y-4">
                <FieldGroup class="gap-4">
                    <VeeField v-slot="{ componentField, errors }" name="currentPassword">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="change-password-current">Current password</FieldLabel>
                            <Input id="change-password-current" type="password" v-bind="componentField"
                                placeholder="Current password" autocomplete="current-password"
                                :aria-invalid="!!errors.length" class="bg-card" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ componentField, errors }" name="newPassword">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="change-password-new">New password</FieldLabel>
                            <Input id="change-password-new" type="password" v-bind="componentField"
                                placeholder="New password" autocomplete="new-password" :aria-invalid="!!errors.length"
                                class="bg-card" />
                            <FieldDescription>
                                At least 8 characters, with an uppercase and a lowercase letter, a number and a
                                special character.
                            </FieldDescription>
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ componentField, errors }" name="confirmPassword">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="change-password-confirm">Confirm new password</FieldLabel>
                            <Input id="change-password-confirm" type="password" v-bind="componentField"
                                placeholder="New password" autocomplete="new-password" :aria-invalid="!!errors.length"
                                class="bg-card" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <FieldError v-if="formErrors.length" :errors="formErrors" />
                </FieldGroup>
            </form>
        </template>
        <template #footer>
            <Button type="submit" form="change-password-form" :disabled="loading" class="w-full h-11 font-bold">
                <Spinner v-if="loading" />
                Change password
            </Button>
        </template>
    </FullScreenLayoutComponent>
</template>
