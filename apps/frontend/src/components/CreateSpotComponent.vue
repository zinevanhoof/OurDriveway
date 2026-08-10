<script setup lang="ts">
import { eurosToCents } from '@/lib/money';
import { ref } from 'vue'
import { useForm } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod';
import { useRouter } from 'vue-router';

import FullScreenLayoutComponent from '@/components/FullScreenLayoutComponent.vue';
import { FieldError } from '@/components/ui/field'
import Button from '@/components/ui/button/Button.vue';
import { Spinner } from '@/components/ui/spinner'

import CreateSpotBasicInfo from '@/components/forms/create-spot-form/CreateSpotBasicInfo.vue';
import CreateSpotAddress from '@/components/forms/create-spot-form/CreateSpotAddress.vue';
import CreateSpotAvailability from '@/components/forms/create-spot-form/CreateSpotAvailability.vue';
import CreateSpotImages from '@/components/forms/create-spot-form/CreateSpotImages.vue';

import type { Availability } from '@/types/domain/spot'
import type { CreateSpotRequest } from '@/types/requests/CreateSpotRequest'
import { createSpot } from '@/api/spotApi'
import { uploadNewImages } from '@/api/mediaApi'

const router = useRouter()

const formSchema = toTypedSchema(
    z.object({
        title: z
            .string()
            .min(5, 'Parking spot title must be at least 5 characters.')
            .max(32, 'Parking spot title must be at most 32 characters.'),
        description: z
            .string()
            .min(20, 'Description must be at least 20 characters.')
            .max(100, 'Description must be at most 100 characters.'),
        pricePerHour: z
            .number()
            .positive('Price per hour must be greater than 0'),

        // The address is verified server-side by LocationIQ, so no length rules here.
        address: z.object({
            line1: z.string(),
            line2: z.string().optional(),
            city: z.string(),
            postalCode: z.string(),
            region: z.string().optional(),
            country: z.string()
        })
    })
)

const { handleSubmit, setValues, setErrors } = useForm({ validationSchema: formSchema })

const availability = ref<Availability>({
    weekly: { monday: [], tuesday: [], wednesday: [], thursday: [], friday: [], saturday: [], sunday: [] },
    single: {},
})
// Mixed-type on create too, so the picker is one component: nothing here ever puts
// a string in it.
const images = ref<(string | File)[]>([])

const loading = ref(false)
const slotErrors = ref<string[]>([])
const imageErrors = ref<string[]>([])
const formErrors = ref<string[]>([])

const hasSlots = () =>
    Object.values(availability.value.weekly).some(slots => slots.length)
    || Object.values(availability.value.single).some(slots => slots.length)

const submit = handleSubmit(async (values) => {
    formErrors.value = []
    slotErrors.value = []
    imageErrors.value = []

    // The schema only covers its own fields; availability and images are guarded here.
    if (!hasSlots())
        slotErrors.value.push('Add at least one availability slot.')
    if (!images.value.length)
        imageErrors.value.push('Add at least one photo.')
    if (slotErrors.value.length || imageErrors.value.length)
        return

    loading.value = true
    try {
        // Photos go to R2 first, and only their keys are sent below — no image
        // bytes reach the backend at all. Uploading before the create means a
        // failure here costs nothing: no spot exists yet to be left half-made.
        const imageKeys = await uploadNewImages(images.value, 'spot')

        const { pricePerHour, ...rest } = values
        const request: CreateSpotRequest = {
            ...rest,
            // The only place euros become cents. Everything server-side is integer.
            pricePerHourCents: eurosToCents(pricePerHour),
            address: {
                ...values.address,
                formatted: [
                    values.address.line1,
                    values.address.line2,
                    [values.address.postalCode, values.address.city].filter(Boolean).join(' '),
                    values.address.region,
                    values.address.country,
                ].filter(Boolean).join(', '),
            },
            availability: availability.value,
            images: imageKeys,
        }

        const response = await createSpot(request)
        if (!response.ok) {
            showServerErrors(await response.json().catch(() => ({})))
            return
        }
        // `refreshSpots` tells SpotsView to refetch instead of serving the cached list.
        router.replace({ name: 'spots', state: { refreshSpots: true } })
    } catch (error) {
        // An upload that never reached R2 leaves nothing behind, so the only thing
        // to do is say so and let them press save again.
        imageErrors.value.push(error instanceof Error ? error.message : 'Upload failed.')
    } finally {
        loading.value = false
    }
})

// Backend field paths are snake_case and only some of them map to a form field.
const toCamel = (k: string) => k.replace(/_([a-z])/g, (_, c) => c.toUpperCase())
const FORM_FIELDS = ['title', 'description', 'pricePerHour', 'address']

const showServerErrors = (body: {
    errors?: Record<string, string[]>,
    detail?: string[],
}) => {
    if (!body.errors) {
        formErrors.value = body.detail ?? ['Something went wrong. Please try again.']
        return
    }

    const fieldErrors: Record<string, string[]> = {}
    for (const [path, messages] of Object.entries(body.errors)) {
        const key = toCamel(path)
        if (key.startsWith('availability'))
            slotErrors.value.push(...messages)
        else if (FORM_FIELDS.some(f => key === f || key.startsWith(`${f}.`)))
            fieldErrors[key] = messages
        else
            formErrors.value.push(...messages)
    }
    setErrors(fieldErrors)
}
</script>

<template>
    <FullScreenLayoutComponent title="New listing" description="List your driveway to start earning"
        @close="router.back()">
        <template #main>
            <form id="create-spot-form" @submit="submit" class="space-y-4">
                <CreateSpotBasicInfo />
                <CreateSpotAddress :set-values="setValues" />
                <CreateSpotAvailability v-model:availability="availability" v-model:slot-errors="slotErrors" />
                <CreateSpotImages v-model:images="images" :image-errors="imageErrors" />
                <FieldError v-if="formErrors.length" :errors="formErrors" />
            </form>
        </template>
        <template #footer>
            <Button type="submit" form="create-spot-form" :disabled="loading" class="w-full h-11 font-bold">
                <Spinner v-if="loading" />
                Publish listing
            </Button>
        </template>
    </FullScreenLayoutComponent>
</template>
