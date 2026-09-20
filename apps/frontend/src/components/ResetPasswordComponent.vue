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

import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { Text, Title } from '@/components/base/text'
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
    <div class="flex flex-1 flex-col gap-9 px-7 pt-24 pb-12">
        <div class="flex flex-col items-center gap-4">
            <img src="/ourdriveway-icon.svg" alt="OurDriveway" width="84" height="84"
                class="size-21 rounded-[20px] shadow-[0_16px_32px_-14px] shadow-primary/50" />
            <Title as="h1" class="text-3xl leading-none tracking-[-0.02em] whitespace-nowrap">
                <span class="font-bold text-primary">Our</span><span class="font-extrabold">Driveway</span>
            </Title>
            <Text as="p" size="md" weight="semibold" class="text-center text-balance">
                <template v-if="dead && formErrors.length">{{ formErrors[0] }}</template>
                <template v-else-if="dead">
                    This link is missing its token. Request a new one below.
                </template>
                <template v-else>Pick something you haven't used here before.</template>
            </Text>
        </div>

        <form v-if="!dead" id="reset-password-form" class="flex flex-col gap-4.5" @submit="submit">
            <!-- The group holds the fields and nothing else. The submit button is a
                 sibling of it, not a member. -->
            <FieldGroup class="gap-4.5">
                <VeeField v-slot="{ field, errors }" name="password">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="reset-password-new">
                            New password
                        </FieldLabel>
                        <Input id="reset-password-new" type="password" v-bind="field" placeholder="••••••••"
                            autocomplete="new-password" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldDescription>
                            At least 8 characters, with an uppercase and a lowercase letter, a number and a
                            special character.
                        </FieldDescription>
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>

                <VeeField v-slot="{ field, errors }" name="confirmPassword">
                    <Field class="gap-2" :data-invalid="!!errors.length">
                        <FieldLabel class="font-bold" for="reset-password-confirm">
                            Confirm new password
                        </FieldLabel>
                        <Input id="reset-password-confirm" type="password" v-bind="field" placeholder="••••••••"
                            autocomplete="new-password" :aria-invalid="!!errors.length" class="h-13 rounded-lg border-2 border-accent px-4 focus-visible:ring-4" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
            </FieldGroup>

            <Button type="submit" :disabled="loading" class="h-13 w-full rounded-lg text-[17px] font-extrabold">
                <Spinner v-if="loading" />
                Set password
            </Button>
        </form>

        <!-- A button, not the footer link: on a dead link this is the only way
             forward, and the footer is where the secondary exit lives. -->
        <Button v-else class="h-13 w-full rounded-lg text-[17px] font-extrabold" @click="router.push({ name: 'forgot-password' })">
            Request a new link
        </Button>

        <p class="mt-auto text-center text-[15px] font-semibold text-muted-foreground">
            Remembered it?
            <RouterLink to="/login" class="font-extrabold text-primary">Log in</RouterLink>
        </p>
    </div>
</template>
