<script setup lang="ts">
// Asks for a reset link. The other half of the flow is
// `ResetPasswordView.vue`, which is where the link lands.
//
// Styled as the third auth screen, alongside login and signup, rather than with
// `FullScreenLayout`: this is reachable by someone with no session, and
// that layout's close button is `router.back()`, which from a fresh tab goes
// nowhere.
import { ref } from 'vue'
import { useForm, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod'

import { Field, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { Text, Title } from '@/components/base/text'
import { useForgotPassword } from '@/api/userApi'
import type { ApiError } from '@/api/client'

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
    <div class="flex flex-1 flex-col gap-9 px-7 pt-24 pb-12">
        <div class="flex flex-col items-center gap-4">
            <img src="/ourdriveway-icon.svg" alt="OurDriveway" width="84" height="84"
                class="size-21 rounded-[20px] shadow-[0_16px_32px_-14px] shadow-primary/50" />
            <Title as="h1" class="text-3xl leading-none tracking-[-0.02em] whitespace-nowrap">
                <span class="font-bold text-primary">Our</span><span class="font-extrabold">Driveway</span>
            </Title>
            <!-- Says nothing about whether the address exists, because the backend
                 deliberately answers the same either way. -->
            <Text as="p" size="md" weight="semibold" class="text-center text-balance">
                <template v-if="sent">
                    If that address has an account, a reset link is on its way. It expires in an
                    hour, and only the most recent link works.
                </template>
                <template v-else>
                    Enter your email and we'll send you a link to set a new password.
                </template>
            </Text>
        </div>

        <form v-if="!sent" id="forgot-password-form" class="flex flex-col gap-4.5" @submit="submit">
            <!-- The group holds the field and nothing else. The form-level error
                 and the submit button are siblings of it, not members. -->
            <FieldGroup class="gap-4.5">
                <VeeField v-slot="{ field, errors }" name="email">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="forgot-password-email">
                            Email
                        </FieldLabel>
                        <Input id="forgot-password-email" type="email" v-bind="field" placeholder="you@example.com"
                            autocomplete="email" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
            </FieldGroup>

            <FieldError v-if="formErrors.length" :errors="formErrors" />

            <Button type="submit" :disabled="loading" class="h-13 w-full rounded-lg text-[17px] font-extrabold">
                <Spinner v-if="loading" />
                Send reset link
            </Button>
        </form>

        <p class="mt-auto text-center text-[15px] font-semibold text-muted-foreground">
            Remembered it?
            <RouterLink to="/login" class="font-extrabold text-primary">Log in</RouterLink>
        </p>
    </div>
</template>
