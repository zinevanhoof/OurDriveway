<script setup lang="ts">
import { useQuery } from '@urql/vue'
import { computed, onScopeDispose, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useForm, useFieldArray, Field as VeeField } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod'
import { Camera, Plus, Trash2 } from '@lucide/vue'

import FullScreenLayoutComponent from '../FullScreenLayoutComponent.vue'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import Avatar from '@/components/ui/avatar/Avatar.vue'
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue'
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue'

import { ME } from '@/api/graphql/user'
import { updateProfile } from '@/api/userApi'
import { fetchMe } from '@/api/me'
import { useAuthStore } from '@/stores/auth'
import { applyValidationErrors, readErrorDetail } from '@/lib/serverErrors'
import { recordId } from '@/lib/utils'

const router = useRouter()
const auth = useAuthStore()

const { data, executeQuery } = useQuery({
    query: ME,
    variables: computed(() => ({ id: recordId(auth.user?.id) })),
})

const me = computed(() => data.value?.user)

// The address the form was loaded with. Changing away from it is what makes the
// password field appear — and what the server independently demands a password
// for, so this is a hint, not the guard.
const loadedEmail = ref('')

const formSchema = toTypedSchema(
    z.object({
        firstName: z.string().min(1, 'First name is required.').max(32, 'First name must be at most 32 characters.'),
        lastName: z.string().min(1, 'Last name is required.').max(32, 'Last name must be at most 32 characters.'),
        email: z.string().email('Enter a valid email address.'),
        licensePlates: z.array(
            z.string()
                .trim()
                .min(1, 'License plate cannot be empty.')
                .max(16, 'License plate must be at most 16 characters.'),
        ),
        currentPassword: z.string().optional(),
    }).superRefine((values, ctx) => {
        // Only a changed email needs it, so an empty box is not an error on a
        // form that never touched the address.
        if (values.email !== loadedEmail.value && !values.currentPassword)
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                message: 'Enter your current password to change your email address.',
                path: ['currentPassword'],
            })
    })
)

const { handleSubmit, setValues, setErrors, values } = useForm({
    validationSchema: formSchema,
    initialValues: { firstName: '', lastName: '', email: '', licensePlates: [], currentPassword: '' },
})

const { fields: plates, push: addPlate, remove: removePlate } = useFieldArray<string>('licensePlates')

const emailChanged = computed(() => !!loadedEmail.value && values.email !== loadedEmail.value)

const loading = ref(false)
const formErrors = ref<string[]>([])

// Prefills once the query lands, and again if it refetches while untouched —
// same pattern as the edit-listing screen.
watch(me, (user) => {
    if (!user) return

    loadedEmail.value = user.email ?? ''
    setValues({
        firstName: user.firstName,
        lastName: user.lastName,
        email: user.email ?? '',
        licensePlates: [...(user.licensePlates ?? [])],
        currentPassword: '',
    })
}, { immediate: true })

// ponytail: preview only — there is no upload route yet, so the picked file is
// never sent and the server is told nothing about the picture. Wire this to a
// multipart PATCH once /api/user/uploads exists (mirror spot-service's
// parse_spot_form). Until then the camera is honest about doing nothing on save.
const picked = ref<string | null>(null)
const revoke = () => picked.value && URL.revokeObjectURL(picked.value)

const onSelectPicture = (event: Event) => {
    const file = (event.target as HTMLInputElement).files?.[0]
    if (!file) return
    revoke()
    picked.value = URL.createObjectURL(file)
}

onScopeDispose(revoke)

const submit = handleSubmit(async (form) => {
    formErrors.value = []
    loading.value = true
    try {
        const response = await updateProfile({
            ...form,
            // Trimmed here because the schema validates the trimmed length but
            // zod's .trim() doesn't rewrite the value bound to the input.
            licensePlates: form.licensePlates.map(p => p.trim()),
            currentPassword: form.currentPassword || undefined,
        })

        if (!response.ok) {
            if (await applyValidationErrors(response, setErrors)) return
            formErrors.value = await readErrorDetail(response)
            return
        }

        // The write went over REST, so there is no mutation response for
        // graphcache to merge — refetch. `recordSeq` already made the request
        // wait for the projection, and normalization spreads the result to every
        // other cached reference to this user.
        await executeQuery({ requestPolicy: 'network-only' })
        // The header reads the store, not the query.
        auth.setUser(await fetchMe())
        router.back()
    } finally {
        loading.value = false
    }
})
</script>

<template>
    <FullScreenLayoutComponent @close="router.back()" title="Edit profile"
        description="Your details and the cars you park">
        <template #main>
            <form id="edit-profile-form" @submit="submit" class="space-y-4">
                <div class="flex justify-center py-2">
                    <Avatar size="3xl" class="relative">
                        <AvatarImage v-if="picked || me?.profilePicture" :src="picked ?? me!.profilePicture" />
                        <AvatarFallback v-if="me"
                            :name="{ firstName: values.firstName || me.firstName, lastName: values.lastName || me.lastName }" />
                        <label
                            class="absolute flex justify-center items-center right-0 bottom-0 bg-card rounded-full w-6 h-6 border border-border shadow-xs cursor-pointer">
                            <Camera :size="16" class="text-primary" />
                            <span class="sr-only">Change profile picture</span>
                            <input type="file" accept="image/*" class="sr-only" @change="onSelectPicture" />
                        </label>
                    </Avatar>
                </div>

                <FieldGroup class="gap-4">
                    <div class="font-bold">Your details</div>

                    <div class="grid grid-cols-2 gap-3">
                        <VeeField v-slot="{ componentField, errors }" name="firstName">
                            <Field :data-invalid="!!errors.length" class="gap-1">
                                <FieldLabel for="edit-profile-firstName">First name</FieldLabel>
                                <Input id="edit-profile-firstName" v-bind="componentField" placeholder="First name"
                                    autocomplete="given-name" :aria-invalid="!!errors.length" class="bg-card" />
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                        <VeeField v-slot="{ componentField, errors }" name="lastName">
                            <Field :data-invalid="!!errors.length" class="gap-1">
                                <FieldLabel for="edit-profile-lastName">Last name</FieldLabel>
                                <Input id="edit-profile-lastName" v-bind="componentField" placeholder="Last name"
                                    autocomplete="family-name" :aria-invalid="!!errors.length" class="bg-card" />
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                    </div>

                    <VeeField v-slot="{ componentField, errors }" name="email">
                        <Field :data-invalid="!!errors.length" class="gap-1">
                            <FieldLabel for="edit-profile-email">Email</FieldLabel>
                            <Input id="edit-profile-email" type="email" v-bind="componentField"
                                placeholder="example@gmail.com" autocomplete="email" :aria-invalid="!!errors.length"
                                class="bg-card" />
                            <FieldDescription>This is what you sign in with.</FieldDescription>
                            <FieldError v-if="errors.length" :errors="errors" />
                        </Field>
                    </VeeField>

                    <div v-auto-animate>
                        <VeeField v-if="emailChanged" v-slot="{ componentField, errors }" name="currentPassword">
                            <Field :data-invalid="!!errors.length" class="gap-1">
                                <FieldLabel for="edit-profile-current-password">Current password</FieldLabel>
                                <Input id="edit-profile-current-password" type="password" v-bind="componentField"
                                    placeholder="Current password" autocomplete="current-password"
                                    :aria-invalid="!!errors.length" class="bg-card" />
                                <FieldDescription>
                                    Confirm it's you before changing the address you sign in with.
                                </FieldDescription>
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                    </div>
                </FieldGroup>

                <FieldGroup class="gap-4">
                    <div>
                        <div class="font-bold">License plates</div>
                        <div class="text-xs text-muted-foreground font-medium">
                            So a host can recognise the car on their driveway.
                        </div>
                    </div>

                    <div v-auto-animate class="space-y-2">
                        <VeeField v-for="(plate, index) in plates" :key="plate.key" v-slot="{ componentField, errors }"
                            :name="`licensePlates[${index}]`">
                            <Field :data-invalid="!!errors.length" class="gap-1">
                                <div class="flex gap-2 items-center">
                                    <Input v-bind="componentField" placeholder="1-ABC-123" autocomplete="off"
                                        :aria-label="`License plate ${index + 1}`" :aria-invalid="!!errors.length"
                                        class="bg-card" />
                                    <Button type="button" variant="outline" size="icon" class="shrink-0"
                                        :aria-label="`Remove license plate ${index + 1}`" @click="removePlate(index)">
                                        <Trash2 class="text-destructive" />
                                    </Button>
                                </div>
                                <FieldError v-if="errors.length" :errors="errors" />
                            </Field>
                        </VeeField>
                    </div>

                    <Button type="button" variant="outline" class="font-bold bg-card" @click="addPlate('')">
                        <Plus />
                        Add license plate
                    </Button>
                </FieldGroup>

                <FieldError v-if="formErrors.length" :errors="formErrors" />
            </form>
        </template>
        <template #footer>
            <Button type="submit" form="edit-profile-form" :disabled="loading" class="w-full h-11 font-bold">
                <Spinner v-if="loading" />
                Save changes
            </Button>
        </template>
    </FullScreenLayoutComponent>
</template>
