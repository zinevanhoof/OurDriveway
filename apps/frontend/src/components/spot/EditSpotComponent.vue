<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query';
import { computed, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { useForm } from 'vee-validate'
import { z } from 'zod'
import { toTypedSchema } from '@vee-validate/zod';

import FullScreenLayoutComponent from '../FullScreenLayoutComponent.vue';
import { FieldError } from '@/components/ui/field'
import Button from '@/components/ui/button/Button.vue';
import { Spinner } from '@/components/ui/spinner'
import { Drawer, DrawerContent } from '@/components/ui/drawer'
import { TriangleAlert } from '@lucide/vue'
import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'

import CreateSpotBasicInfo from '@/components/forms/create-spot-form/CreateSpotBasicInfo.vue';
import CreateSpotAvailability from '@/components/forms/create-spot-form/CreateSpotAvailability.vue';
import CreateSpotImages from '@/components/forms/create-spot-form/CreateSpotImages.vue';

import { fetchHostSpot } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import { useDeleteSpot, useUpdateSpot } from '@/api/spotApi';
import type { ApiError } from '@/api/client';
import { uploadNewImages } from '@/api/mediaApi';
import { bookedOutside, mergeBooked } from '@/lib/bookingAvailability';
import { formatDay, formatSlots, todayIn } from '@/lib/bookingDates';
import { centsToEuros, eurosToCents } from '@/lib/money';
import type { Availability } from '@/types/domain/spot'

const { id } = defineProps<{ id: string }>()

const router = useRouter()


const { data } = useQuery({
    queryKey: viewKeys.hostSpot(id),
    queryFn: () => fetchHostSpot(id),
})

// Same rules as the create form, minus the address: a spot's location is fixed at
// creation, because every stored booking time is wall-clock in the zone derived
// from it.
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
    })
)

const { handleSubmit, setValues, setErrors } = useForm({ validationSchema: formSchema })

// Declared before the watcher below, not after it: with `immediate: true` that
// watcher runs during setup, so anything it calls has to already be initialized.
// A cached spot makes it fire synchronously, which is why this only broke on the
// second visit to the screen.
const emptyWeek = (): Availability['weekly'] =>
    ({ monday: [], tuesday: [], wednesday: [], thursday: [], friday: [], saturday: [], sunday: [] })

const availability = ref<Availability>({ weekly: emptyWeek(), single: {} })
const images = ref<(string | File)[]>([])

const { mutateAsync: save, isPending: loading } = useUpdateSpot()
const { mutateAsync: destroy, isPending: deleting } = useDeleteSpot()
const confirmOpen = ref(false)
const slotErrors = ref<string[]>([])
const imageErrors = ref<string[]>([])
const formErrors = ref<string[]>([])

const timezone = computed(() => data.value?.timezone)
const today = computed(() => todayIn(timezone.value))

// Prefills once the query lands, and again if it refetches while untouched. The
// spot itself is the source of truth for the initial state; everything after is
// the host's edit.
watch(() => data.value, (spot) => {
    if (!spot) return

    setValues({
        title: spot.title,
        description: spot.description ?? '',
        pricePerHour: centsToEuros(spot.pricePerHour),
    })

    availability.value = {
        weekly: { ...emptyWeek(), ...(spot.availability?.weekly ?? {}) },
        // Past one-off dates are dropped rather than shown: the server rejects a
        // date before today on *any* submit, so keeping them would make a form the
        // host never touched unsavable.
        single: Object.fromEntries(
            Object.entries(spot.availability?.single ?? {})
                .filter(([date]) => date >= today.value),
        ) as Availability['single'],
    }

    images.value = [...(spot.images ?? [])]
}, { immediate: true })

const hasSlots = () =>
    Object.values(availability.value.weekly).some(slots => slots.length)
    || Object.values(availability.value.single).some(slots => slots.length)

/**
 * Booked slots the host is about to remove the hours from.
 *
 * Shown before they save, because saving cancels those bookings and refunds them —
 * booking-service does that when the event lands, and it is the authority. This is
 * only the warning that it is about to happen.
 */
const casualties = computed(() =>
    bookedOutside(availability.value, mergeBooked(data.value?.bookings), today.value))

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

    // Which half of the try threw: an upload failure belongs under the picker.
    let uploading = false
    try {
        // Kept photos are already keys and pass straight through; only the newly
        // picked Files are uploaded. One list, in the host's display order, so the
        // server never has to work out what changed.
        uploading = true
        const imageUrls = await uploadNewImages(images.value, 'spot')
        uploading = false

        const { pricePerHour, ...rest } = values
        await save({
            spotId: id,
            body: {
                ...rest,
                pricePerHourCents: eurosToCents(pricePerHour),
                availability: availability.value,
                images: imageUrls,
            },
        })
        router.back()
    } catch (e) {
        const err = e as ApiError
        // A failed upload leaves the listing exactly as it was — nothing was saved.
        if (uploading) imageErrors.value.push(err.detail[0])
        else showServerErrors(err)
    }
})

const remove = async () => {
    try {
        await destroy(id)
        // Past the manage screen, which is about to 404 on a spot that no longer
        // lists. `refreshSpots` makes the list refetch instead of serving its cache.
        router.replace({ name: 'spots', state: { refreshSpots: true } })
    } catch (e) {
        confirmOpen.value = false
        formErrors.value = (e as ApiError).detail
    }
}

// Only some backend field paths map to a form field; availability's belong under
// the slot picker. The paths arrive camelCase — the server converts garde's
// snake_case ones now, so the `toCamel` that used to be here is gone.
const FORM_FIELDS = ['title', 'description', 'pricePerHour']

const showServerErrors = (err: ApiError) => {
    if (Object.keys(err.fields).length === 0) {
        formErrors.value = err.detail
        return
    }

    const fieldErrors: Record<string, string[]> = {}
    for (const [key, messages] of Object.entries(err.fields)) {
        if (key.startsWith('availability'))
            slotErrors.value.push(...messages)
        else if (key.startsWith('images'))
            imageErrors.value.push(...messages)
        else if (FORM_FIELDS.some(f => key === f || key.startsWith(`${f}.`)))
            fieldErrors[key] = messages
        else
            formErrors.value.push(...messages)
    }
    setErrors(fieldErrors)
}
</script>

<template>
    <FullScreenLayoutComponent @close="router.back()" title="Edit listing" :description="data?.title">
        <template #main>
            <form id="edit-spot-form" @submit="submit" class="space-y-4">
                <CreateSpotBasicInfo />
                <CreateSpotAvailability v-model:availability="availability" v-model:slot-errors="slotErrors" />
                <CreateSpotImages v-model:images="images" :image-errors="imageErrors" />

                <div v-auto-animate>
                    <Surface v-if="casualties.length" variant="destructive" orientation="horizontal"
                        class="items-start gap-2 text-sm">
                        <TriangleAlert class="size-4 shrink-0 text-destructive mt-0.5" />
                        <div class="space-y-1">
                            <Title size="sm">
                                {{ casualties.length }} booked
                                {{ casualties.length === 1 ? 'slot falls' : 'slots fall' }} outside your new
                                hours
                            </Title>
                            <Text>
                                Saving cancels
                                {{ casualties.length === 1 ? 'it' : 'them' }} and refunds the renter.
                            </Text>
                            <Text v-for="({ date, slot }) in casualties" :key="`${date}-${slot.start}`"
                                weight="semibold" tone="default">
                                {{ formatDay(date, timezone) }} · {{ formatSlots([slot]) }}
                            </Text>
                        </div>
                    </Surface>
                </div>

                <FieldError v-if="formErrors.length" :errors="formErrors" />

                <Button type="button" variant="outline" @click="confirmOpen = true"
                    class="w-full h-11 font-bold text-destructive">
                    Delete listing
                </Button>
            </form>
        </template>
        <template #footer>
            <Button type="submit" form="edit-spot-form" :disabled="loading" class="w-full h-11 font-bold">
                <Spinner v-if="loading" />
                Save changes
            </Button>
        </template>
    </FullScreenLayoutComponent>

    <!-- Defaults left alone on purpose: drag-to-dismiss, the handle, backdrop tap
         and Esc are all vaul's, and a confirmation is the last place to break the
         gesture someone already expects. -->
    <Drawer v-model:open="confirmOpen">
        <DrawerContent @close-auto-focus.prevent
            class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
            <div class="m-4 space-y-4">
                <div>
                    <Title size="lg">Delete this listing?</Title>
                    <Text size="sm">
                        {{ data?.title }} comes off the market for good. Any booking it still
                        owes is cancelled and refunded. This can't be undone.
                    </Text>
                </div>
                <div class="space-y-2">
                    <Button variant="destructive" class="w-full h-11 font-bold" :disabled="deleting" @click="remove">
                        {{ deleting ? "Deleting…" : "Yes, delete it" }}
                    </Button>
                    <Button variant="outline" class="w-full h-11 font-bold" @click="confirmOpen = false">
                        Keep listing
                    </Button>
                </div>
            </div>
        </DrawerContent>
    </Drawer>
</template>
