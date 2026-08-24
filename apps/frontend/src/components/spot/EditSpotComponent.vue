<script setup lang="ts">
import { useQuery } from '@urql/vue';
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

import CreateSpotBasicInfo from '@/components/forms/create-spot-form/CreateSpotBasicInfo.vue';
import CreateSpotAvailability from '@/components/forms/create-spot-form/CreateSpotAvailability.vue';
import CreateSpotImages from '@/components/forms/create-spot-form/CreateSpotImages.vue';

import { EDIT_SPOT } from '@/api/graphql/spot';
import { deleteSpot, updateSpot } from '@/api/spotApi';
import { uploadNewImages } from '@/api/mediaApi';
import { bookedOutside, mergeBooked } from '@/lib/bookingAvailability';
import { formatDay, formatSlots, todayIn } from '@/lib/bookingDates';
import { centsToEuros, eurosToCents } from '@/lib/money';
import { gqlRecordId, plainUuid } from '@/lib/utils';
import type { Availability } from '@/types/domain/spot'

const { id } = defineProps<{ id: string }>()

const router = useRouter()

const { data } = useQuery({
    query: EDIT_SPOT,
    variables: computed(() => ({
        id: gqlRecordId(id),
        spotUuid: plainUuid(id),
        now: new Date().toISOString(),
    })),
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

const loading = ref(false)
const deleting = ref(false)
const confirmOpen = ref(false)
const slotErrors = ref<string[]>([])
const imageErrors = ref<string[]>([])
const formErrors = ref<string[]>([])

const timezone = computed(() => data.value?.spot?.timezone)
const today = computed(() => todayIn(timezone.value))

// Prefills once the query lands, and again if it refetches while untouched. The
// spot itself is the source of truth for the initial state; everything after is
// the host's edit.
watch(() => data.value?.spot, (spot) => {
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

    loading.value = true
    try {
        // Kept photos are already keys and pass straight through; only the newly
        // picked Files are uploaded. One list, in the host's display order, so the
        // server never has to work out what changed.
        const imageKeys = await uploadNewImages(images.value, 'spot')

        const { pricePerHour, ...rest } = values
        const response = await updateSpot(plainUuid(id)!, {
            ...rest,
            pricePerHourCents: eurosToCents(pricePerHour),
            availability: availability.value,
            images: imageKeys,
        })
        if (!response.ok) {
            showServerErrors(await response.json().catch(() => ({})))
            return
        }
        router.back()
    } catch (error) {
        // A failed upload leaves the listing exactly as it was — nothing was saved.
        imageErrors.value.push(error instanceof Error ? error.message : 'Upload failed.')
    } finally {
        loading.value = false
    }
})

const remove = async () => {
    deleting.value = true
    try {
        await deleteSpot(plainUuid(id)!)
        // Past the manage screen, which is about to 404 on a spot that no longer
        // lists. `refreshSpots` makes the list refetch instead of serving its cache.
        router.replace({ name: 'spots', state: { refreshSpots: true } })
    } catch (e) {
        confirmOpen.value = false
        formErrors.value = [e instanceof Error ? e.message : 'Something went wrong. Please try again.']
    } finally {
        deleting.value = false
    }
}

// Backend field paths are snake_case and only some of them map to a form field.
const toCamel = (k: string) => k.replace(/_([a-z])/g, (_, c) => c.toUpperCase())
const FORM_FIELDS = ['title', 'description', 'pricePerHour']

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
    <FullScreenLayoutComponent @close="router.back()" title="Edit listing" :description="data?.spot?.title">
        <template #main>
            <form id="edit-spot-form" @submit="submit" class="space-y-4">
                <CreateSpotBasicInfo />
                <CreateSpotAvailability v-model:availability="availability" v-model:slot-errors="slotErrors" />
                <CreateSpotImages v-model:images="images" :image-errors="imageErrors" />

                <div v-auto-animate>
                    <div v-if="casualties.length"
                        class="flex gap-2 p-3 text-sm rounded-md border border-destructive/40 bg-destructive/5">
                        <TriangleAlert class="size-4 shrink-0 text-destructive mt-0.5" />
                        <div class="space-y-1">
                            <div class="font-bold">
                                {{ casualties.length }} booked
                                {{ casualties.length === 1 ? 'slot falls' : 'slots fall' }} outside your new
                                hours
                            </div>
                            <div class="text-xs text-muted-foreground font-medium">
                                Saving cancels
                                {{ casualties.length === 1 ? 'it' : 'them' }} and refunds the renter.
                            </div>
                            <div v-for="({ date, slot }) in casualties" :key="`${date}-${slot.start}`"
                                class="text-xs font-semibold">
                                {{ formatDay(date, timezone) }} · {{ formatSlots([slot]) }}
                            </div>
                        </div>
                    </div>
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
                    <div class="text-lg font-bold">Delete this listing?</div>
                    <div class="text-sm text-muted-foreground font-medium">
                        {{ data?.spot?.title }} comes off the market for good. Any booking it still
                        owes is cancelled and refunded. This can't be undone.
                    </div>
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
