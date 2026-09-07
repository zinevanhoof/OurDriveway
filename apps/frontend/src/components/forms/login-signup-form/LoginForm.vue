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
import { loginUser, resendVerification } from '@/api/userApi'
import { applyValidationErrors, readErrorDetail } from '@/lib/serverErrors'
import { AuthResponse } from '@/types/response/AuthResponse'
import { toast } from 'vue-sonner'

const emit = defineEmits<{
    success: [AuthResponse]
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
const resending = ref(false)

const onSubmit = handleSubmit(async (data) => {
    serverErrors.value = []
    unverified.value = false
    const response = await loginUser(data)

    if (response.ok) {
        const auth: AuthResponse = await response.json()
        emit('success', auth)
        return
    }

    if (response.status === 403) {
        unverified.value = true
        return
    }

    // 422 -> per-field errors; anything else (e.g. 401 invalid credentials) -> form-level detail
    if (await applyValidationErrors(response, setErrors)) return
    serverErrors.value = await readErrorDetail(response)
})

const resend = async () => {
    resending.value = true
    await resendVerification(values.email ?? '')
    resending.value = false
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
