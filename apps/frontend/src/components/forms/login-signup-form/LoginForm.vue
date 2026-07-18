<script setup lang="ts">
import { toTypedSchema } from '@vee-validate/zod'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { ref } from 'vue'

import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardFooter,
    CardTitle,
} from '@/components/ui/card'
import {
    Field,
    FieldError,
    FieldGroup,
    FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { loginUser } from '@/api/userApi'
import { applyValidationErrors, readErrorDetail } from '@/lib/serverErrors'
import { AuthResponse } from '@/types/response/AuthResponse'

const emit = defineEmits<{
    success: [AuthResponse]
}>()

const formSchema = toTypedSchema(
    z.object({
        email: z.string().email(),
        password: z.string()
    })
)

const { handleSubmit, setErrors, isSubmitting } = useForm({
    validationSchema: formSchema,
    initialValues: {
        email: '',
        password: ''
    },
})

const serverErrors = ref<string[]>([])

const onSubmit = handleSubmit(async (data) => {
    serverErrors.value = []
    const response = await loginUser(data)

    if (response.ok) {
        const auth: AuthResponse = await response.json()
        emit('success', auth)
        return
    }

    // 422 -> per-field errors; anything else (e.g. 401 invalid credentials) -> form-level detail
    if (await applyValidationErrors(response, setErrors)) return
    serverErrors.value = await readErrorDetail(response)
})
</script>

<template>
    <Card>
        <CardHeader>
            <CardTitle>Login</CardTitle>
            <CardDescription>
                Login into your account here
            </CardDescription>
        </CardHeader>
        <CardContent>
            <form id="form-login" @submit="onSubmit">
                <FieldGroup>
                    <VeeField v-slot="{ field, errors }" name="email">
                        <Field :data-invalid="!!errors.length">
                            <FieldLabel for="form-login-email">
                                Email
                            </FieldLabel>
                            <Input id="form-login-email" v-bind="field" placeholder="example@gmail.com"
                                autocomplete="off" :aria-invalid="!!errors.length" />
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
                </FieldGroup>
            </form>
        </CardContent>
        <CardFooter>
            <Button class="flex-1" type="submit" form="form-login" :disabled="isSubmitting">
                Login
            </Button>
        </CardFooter>
    </Card>
</template>
