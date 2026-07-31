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
import { signupUser } from '@/api/userApi'
import { applyValidationErrors, readErrorDetail } from '@/lib/serverErrors'

const emit = defineEmits<{
    success: []
}>()

const formSchema = toTypedSchema(
    z.object({
        firstName: z.string(),
        lastName: z.string(),
        email: z.string().email(),
        password: z
            .string()
            .min(8, "Password must be at least 8 characters")
            .max(32, "Password must be at most 32 characters")
            .regex(/[A-Z]/, "Must contain at least one uppercase letter")
            .regex(/[a-z]/, "Must contain at least one lowercase letter")
            .regex(/[0-9]/, "Must contain at least one number")
            .regex(/[^A-Za-z0-9]/, "Must contain at least one special character"),

        confirmPassword: z.string(),
    }).refine((data) => data.password === data.confirmPassword, {
        message: "Passwords do not match",
        path: ["confirmPassword"],
    })
)

const { handleSubmit, setErrors, isSubmitting } = useForm({
    validationSchema: formSchema,
    initialValues: {
        firstName: '',
        lastName: '',
        email: '',
        password: '',
        confirmPassword: ''
    },
})

const serverErrors = ref<string[]>([])

const onSubmit = handleSubmit(async ({ confirmPassword, ...form }) => {
    serverErrors.value = []
    const response = await signupUser(form)

    if (response.ok) {
        emit('success')
        return
    }

    if (await applyValidationErrors(response, setErrors)) return
    serverErrors.value = await readErrorDetail(response)
})
</script>

<template>
    <Card>
        <CardHeader>
            <CardTitle>Signup</CardTitle>
            <CardDescription>
                Create an account here
            </CardDescription>
        </CardHeader>
        <CardContent>
            <form id="form-register" @submit="onSubmit">
                <FieldGroup>
                    <div class="flex justify-center gap-2 items-center">
                        <VeeField v-slot="{ field, errors }" name="firstName">
                            <Field :data-invalid="!!errors.length">
                                <FieldLabel for="form-register-firstName">
                                    First name
                                </FieldLabel>
                                <Input id="form-register-firstName" v-bind="field" placeholder="First name"
                                    autocomplete="off" :aria-invalid="!!errors.length" />
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                        <VeeField v-slot="{ field, errors }" name="lastName">
                            <Field :data-invalid="!!errors.length">
                                <FieldLabel for="form-register-lastName">
                                    Last name
                                </FieldLabel>
                                <Input id="form-register-lastName" v-bind="field" placeholder="Last name"
                                    autocomplete="off" :aria-invalid="!!errors.length" />
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                    </div>
                    <VeeField v-slot="{ field, errors }" name="email">
                        <Field :data-invalid="!!errors.length">
                            <FieldLabel for="form-register-email">
                                Email
                            </FieldLabel>
                            <Input id="form-register-email" v-bind="field" placeholder="example@gmail.com"
                                autocomplete="off" :aria-invalid="!!errors.length" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ field, errors }" name="password">
                        <Field :data-invalid="!!errors.length">
                            <FieldLabel for="form-register-password">
                                Password
                            </FieldLabel>
                            <Input type="password" id="form-register-password" v-bind="field" placeholder="Password"
                                autocomplete="off" :aria-invalid="!!errors.length" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ field, errors }" name="confirmPassword">
                        <Field :data-invalid="!!errors.length">
                            <FieldLabel for="form-register-confirm-password">
                                Confirm password
                            </FieldLabel>
                            <Input type="password" id="form-register-confirm-password" v-bind="field"
                                placeholder="Password" autocomplete="off" :aria-invalid="!!errors.length" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <FieldError v-if="serverErrors.length" :errors="serverErrors" />
                </FieldGroup>
            </form>
        </CardContent>
        <CardFooter>
            <Button class="flex-1" type="submit" form="form-register" :disabled="isSubmitting">
                Signup
            </Button>
        </CardFooter>
    </Card>
</template>
