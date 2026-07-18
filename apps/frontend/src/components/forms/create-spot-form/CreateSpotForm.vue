<script setup lang="ts">
import { z } from 'zod'
import { computed, nextTick, ref } from 'vue';
import { FieldError } from '@/components/ui/field'
import CreateSpotBasicInfo from './CreateSpotBasicInfo.vue';
import CreateSpotAddress from './CreateSpotAddress.vue';
import Button from '@/components/ui/button/Button.vue';
import { Spinner } from '@/components/ui/spinner'
import CreateSpotWeekdayAvailability from './CreateSpotWeekdayAvailability.vue';
import CreateSpotSingleAvailability from './CreateSpotSingleAvailability.vue';
import { CreateSpotRequest } from '@/types/requests/CreateSpotRequest.ts';

import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardFooter,
    CardTitle,
} from '@/components/ui/card'
import { Form } from 'vee-validate';
import { toTypedSchema } from '@vee-validate/zod';
import CreateSpotSelectImages from './CreateSpotSelectImages.vue';

const props = defineProps<{ loading?: boolean }>()

const currentStep = ref<number>(0)
const veeForm = ref()
const serverErrors = ref<string[]>([])

const form = ref<CreateSpotRequest>({
    title: '',
    description: '',
    pricePerHour: 0,
    address: {
        line1: '',
        city: '',
        postalCode: '',
        country: '',
        formatted: ''
    },
    availability: {
        weekly: {
            monday: [],
            tuesday: [],
            wednesday: [],
            thursday: [],
            friday: [],
            saturday: [],
            sunday: []
        },
        single: {}
    }
})

const images = ref<File[]>([])

const cardDescriptions = [
    'Basic information',
    'Address',
    'Weekly availability',
    'Single day availability',
    'Select images'
]

const schemas = [
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
            .positive('Price per hour must be greater than 0')
    }),
    z.object({
        // The address is verified server-side by LocationIQ, so no length rules here.
        address: z.object({
            line1: z.string(),
            line2: z.string().optional(),
            city: z.string(),
            postalCode: z.string(),
            region: z.string().optional(),
            country: z.string()
        })
    }),
    z.object({}),
    z.object({}),
    z.object({})
]

const currentSchema = computed(() => {
    return toTypedSchema(schemas[currentStep.value]);
});

const nextStep = (data: any) => {
    serverErrors.value = []
    if (currentStep.value === 0) {
        form.value.title = data.title
        form.value.description = data.description
        form.value.pricePerHour = data.pricePerHour
    } else if (currentStep.value === 1) {
        form.value.address = data.address
        form.value.address.formatted = [
            data.address.line1,
            data.address.line2,
            [data.address.postalCode, data.address.city].filter(Boolean).join(' '),
            data.address.region,
            data.address.country
        ].filter(Boolean).join(', ')
    } else if (currentStep.value > 3) {
        emit('submit', { form: form.value, images: images.value })
        return;
    }

    currentStep.value++;
}

const prevStep = () => {
    serverErrors.value = []
    currentStep.value--
}

// Backend field paths -> which step owns them.
const stepOfKey = (k: string) =>
    k.startsWith('address') ? 1 :
        k.startsWith('availability.weekly') ? 2 :
            k.startsWith('availability.single') ? 3 :
                k.startsWith('availability') ? 2 : 0 // title / description / price_per_hour
const toCamel = (k: string) => k.replace(/_([a-z])/g, (_, c) => c.toUpperCase())

// Called by the parent after a non-ok create response: jump to the offending
// step and surface the messages (inline on step 0, a list elsewhere).
const showServerErrors = async (body: {
    errors?: Record<string, string[]>,
    detail?: string[],
    title?: string,
}) => {
    serverErrors.value = []
    if (body.errors) {
        const keys = Object.keys(body.errors)
        const step = Math.min(...keys.map(stepOfKey))
        currentStep.value = step
        await nextTick() // let the target step mount before setting field errors
        if (step === 0) {
            const fieldErrors: Record<string, string[]> = {}
            for (const [k, msgs] of Object.entries(body.errors))
                if (stepOfKey(k) === 0) fieldErrors[toCamel(k)] = msgs
            veeForm.value?.setErrors(fieldErrors)
        } else {
            serverErrors.value = keys
                .filter(k => stepOfKey(k) === step)
                .flatMap(k => body.errors![k])
        }
    } else if (body.title === 'Address could not be verified') {
        currentStep.value = 1
        serverErrors.value = body.detail ?? ['Address could not be verified.']
    } else {
        serverErrors.value = body.detail ?? ['Something went wrong. Please try again.']
    }
}

defineExpose({ showServerErrors })

type CreateSpotFormSubmit = {
    form: CreateSpotRequest,
    images: File[]
}

const emit = defineEmits<{
    submit: [CreateSpotFormSubmit]
}>()
</script>

<template>
    <Card class="ring-0 shadow-none">
        <CardHeader>
            <CardTitle>Create parking spot</CardTitle>
            <CardDescription>
                {{ cardDescriptions[currentStep] }}
            </CardDescription>
        </CardHeader>
        <CardContent class="max-h-80 overflow-y-auto">
            <Form ref="veeForm" id="create-spot-form" v-slot="{ setValues }" @submit="nextStep"
                :validation-schema="currentSchema" keep-values>
                <CreateSpotBasicInfo v-if="currentStep === 0" />
                <CreateSpotAddress v-if="currentStep === 1" :set-values="setValues" />
                <CreateSpotWeekdayAvailability :form="form" v-if="currentStep === 2" />
                <CreateSpotSingleAvailability :form="form" v-if="currentStep === 3" />
                <CreateSpotSelectImages :images="images" v-if="currentStep === 4" />
                <FieldError v-if="serverErrors.length" :errors="serverErrors" class="mt-2" />
            </Form>
        </CardContent>
        <CardFooter class="gap-2">
            <Button v-if="currentStep !== 0" @click="prevStep" class="flex-1">
                Back
            </Button>
            <Button class="flex-1" type="submit" form="create-spot-form" :disabled="loading">
                <Spinner v-if="loading" />
                {{ currentStep === 4 ? 'Finish' : 'Next' }}
            </Button>
        </CardFooter>
    </Card>
</template>