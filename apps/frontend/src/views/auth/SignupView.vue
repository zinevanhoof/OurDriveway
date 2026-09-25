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
import { useSignup } from '@/api/userApi'
import { passwordRules } from '@/lib/passwordSchema'
import type { ApiError } from '@/api/client'

const router = useRouter()

const formSchema = toTypedSchema(
    z.object({
        firstName: z.string(),
        lastName: z.string(),
        email: z.string().email(),
        password: passwordRules,
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

const { mutateAsync: signup } = useSignup()

const onSubmit = handleSubmit(async ({ confirmPassword, ...form }) => {
    serverErrors.value = []

    try {
        await signup(form)
        // No session is issued at signup and login will refuse until the address
        // is confirmed, so this has to say what happens next rather than "you're
        // in" — hence the login screen plus an instruction, not the home screen.
        router.push({ name: 'login' })
        toast.success('Check your email to verify your address, then log in.')
    } catch (e) {
        const err = e as ApiError
        if (!err.applyTo(setErrors)) serverErrors.value = err.detail
    }
})
</script>

<template>
    <div class="flex flex-1 flex-col gap-9 px-7 pt-24 pb-12">
        <div class="flex flex-col items-center gap-4">
            <img src="/ourdriveway-icon.svg" alt="OurDriveway" width="84" height="84"
                class="size-21 rounded-[20px] shadow-[0_16px_32px_-14px] shadow-primary/50" />
            <Title as="h1" class="text-3xl leading-none tracking-[-0.02em] whitespace-nowrap">
                <span class="font-bold text-primary">Our</span><span class="font-extrabold">Driveway</span>
            </Title>
            <Text as="p" size="md" weight="semibold" class="text-center">Create your account</Text>
        </div>

        <form id="form-signup" class="flex flex-col gap-4.5" @submit="onSubmit">
            <!-- The group holds the fields and nothing else. The form-level error
                 and the submit button are siblings of it, not members. -->
            <FieldGroup class="gap-4.5">
                <!-- `items-start`, not `items-center`: an error under one name
                     would otherwise shove the other input out of line. -->
                <div class="grid grid-cols-2 items-start gap-3">
                    <VeeField v-slot="{ field, errors }" name="firstName">
                        <Field class="gap-2" :data-invalid="!!errors.length">
                            <FieldLabel class="font-bold" for="form-signup-first-name">
                                First name
                            </FieldLabel>
                            <Input id="form-signup-first-name" v-bind="field" placeholder="Jane"
                                autocomplete="given-name" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <VeeField v-slot="{ field, errors }" name="lastName">
                        <Field class="gap-2" :data-invalid="!!errors.length">
                            <FieldLabel class="font-bold" for="form-signup-last-name">
                                Last name
                            </FieldLabel>
                            <Input id="form-signup-last-name" v-bind="field" placeholder="Doe"
                                autocomplete="family-name" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>
                </div>

                <VeeField v-slot="{ field, errors }" name="email">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="form-signup-email">
                            Email
                        </FieldLabel>
                        <Input id="form-signup-email" type="email" v-bind="field" placeholder="you@example.com"
                            autocomplete="email" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <VeeField v-slot="{ field, errors }" name="password">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="form-signup-password">
                            Password
                        </FieldLabel>
                        <Input id="form-signup-password" type="password" v-bind="field" placeholder="••••••••"
                            autocomplete="new-password" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <VeeField v-slot="{ field, errors }" name="confirmPassword">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="form-signup-confirm-password">
                            Confirm password
                        </FieldLabel>
                        <Input id="form-signup-confirm-password" type="password" v-bind="field" placeholder="••••••••"
                            autocomplete="new-password" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
            </FieldGroup>

            <FieldError v-if="serverErrors.length" :errors="serverErrors" />

            <Button type="submit" :disabled="isSubmitting" class="h-13 w-full rounded-lg text-[17px] font-extrabold">
                Create account
            </Button>
        </form>

        <p class="mt-auto text-center text-[15px] font-semibold text-muted-foreground">
            Already have an account?
            <RouterLink to="/login" class="font-extrabold text-primary">Log in</RouterLink>
        </p>
    </div>
</template>
