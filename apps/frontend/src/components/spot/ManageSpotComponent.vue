<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query';
import Button from '../ui/button/Button.vue';
import { fetchHostSpot } from '@/api/viewApi';
import { viewKeys } from '@/api/keys';
import type { HostBookingResponse } from '@/types/responses/view/HostBookingResponse';
import { computed, ref } from 'vue';
import { ArrowLeft, Pencil, Star, Trash2 } from '@lucide/vue';
import { useRouter } from 'vue-router';
import { formatDay, formatSlots, sortedDays, todayIn } from '@/lib/bookingDates.ts';
import { useDeleteSpot, useUpdateSpot } from '@/api/spotApi';
import type { ApiError } from '@/api/client';
import type { TimeSlot, WeeklyAvailability } from '@/types/domain/spot';

import Avatar from "../ui/avatar/Avatar.vue";
import AvatarImage from "../ui/avatar/AvatarImage.vue";
import AvatarFallback from "../ui/avatar/AvatarFallback.vue";
import Switch from '../ui/switch/Switch.vue';
import { Drawer, DrawerContent } from '@/components/ui/drawer'
import { FieldError } from '@/components/ui/field'
import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { Money } from '@/components/base/money'
import { SectionHeader } from '@/components/base/section-header'

const { id } = defineProps<{ id: string }>()

const router = useRouter()


// One request, one path parameter. `MANAGE_SPOT` needed three variables — the spot in
// two different spellings plus a `now` for the bookings filter — and the "read once per
// mount, not per render" note that went with the clock: the server holds the instant
// now, so there is no reactive `now` to accidentally refetch on.
//
// The same key as every other reader of this spot, so the detail drawer and the booking
// form share one cached copy.
const { data } = useQuery({
    queryKey: viewKeys.hostSpot(id),
    queryFn: () => fetchHostSpot(id),
})

const routeToSpotEdit = () => router.push({ name: 'spot-edit', params: { id } })

const timezone = computed(() => data.value?.timezone)

// ─── listing is live ────────────────────────────────────────────────────────

// Optimistic: the switch moves now and the projection catches up in a moment.
// `pending` holds what we asked for until a refetch confirms it, so the thumb
// doesn't snap back to a stale read.
const pending = ref<boolean>()
const live = computed(() => pending.value ?? data.value?.active ?? false)
const errors = ref<string[]>([])

// `{ active }` and nothing else: an edit only touches the fields it carries, and a
// toggle that also resubmitted availability would run the backend's
// cancel-what-no-longer-fits pass off a read that may be a moment stale.
const { mutateAsync: save } = useUpdateSpot()

const toggleLive = async (active: boolean) => {
    pending.value = active
    errors.value = []
    try {
        await save({ spotId: id, body: { active } })
    } catch (e) {
        errors.value = (e as ApiError).detail
    } finally {
        pending.value = undefined
    }
}

// ─── delete ─────────────────────────────────────────────────────────────────

const confirmOpen = ref(false)
const { mutateAsync: destroy, isPending: deleting } = useDeleteSpot()

const remove = async () => {
    try {
        await destroy(id)
        router.replace({ name: 'spots', state: { refreshSpots: true } })
    } catch (e) {
        confirmOpen.value = false
        errors.value = (e as ApiError).detail
    }
}

// ─── availability ───────────────────────────────────────────────────────────

const WEEKDAYS: (keyof WeeklyAvailability)[] =
    ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday']

type Row = { key: string, label: string, slots: TimeSlot[] }

// Days the host actually opens. An empty weekday is not a row — it would be six
// lines of "nothing" on a spot that only opens Saturdays.
const weekly = computed<Row[]>(() =>
    WEEKDAYS
        .map(day => ({
            key: day,
            label: day[0].toUpperCase() + day.slice(1),
            slots: (data.value?.availability?.weekly?.[day] ?? []) as TimeSlot[],
        }))
        .filter(row => row.slots.length))

// Past one-off dates are dropped: they can't be booked, and they'd pile up forever.
const single = computed<Row[]>(() => {
    const today = todayIn(timezone.value)
    return Object.entries(data.value?.availability?.single ?? {})
        .filter(([date, slots]) => date >= today && (slots as TimeSlot[]).length)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([date, slots]) => ({
            key: date,
            label: formatDay(date, timezone.value),
            slots: slots as TimeSlot[],
        }))
})

// One open row at a time, keyed by row — tapping a day with more than one slot
// expands the rest rather than opening a screen for two lines of text.
const expanded = ref<string>()
const toggle = (key: string) => expanded.value = expanded.value === key ? undefined : key

const slotsShown = (row: Row) =>
    expanded.value === row.key ? row.slots : row.slots.slice(0, 1)

// The two lists differ only in their heading and how they build a label, so they
// render through one loop rather than two near-identical blocks.
const sections = computed(() => [
    { title: 'Weekly availability', rows: weekly.value, empty: 'No repeating hours yet.' },
    { title: 'One-off dates', rows: single.value, empty: 'No extra dates coming up.' },
])

// ─── bookings ───────────────────────────────────────────────────────────────

const PREVIEW = 2
const showAllBookings = ref(false)

// Already filtered to the future by the query's ends_at comparison; sorted here
// because "soonest first" is a presentation choice, and the list is two rows long
// until someone asks for all of it.
// The host is a party to every booking on their own spot, so `renter` and `amount`
// come back populated here — the same rows read by a stranger would have both null.
const upcoming = computed<HostBookingResponse[]>(() =>
    [...(data.value?.bookings ?? [])]
        .filter((b) => b.status === 'confirmed' || b.status === 'reserved')
        .sort((a, b) => a.endsAt.localeCompare(b.endsAt)))

const visibleBookings = computed(() =>
    showAllBookings.value ? upcoming.value : upcoming.value.slice(0, PREVIEW))

/** "Mon, Aug 3 · 09:00–10:00", plus a count when the booking spans more days. */
const bookingWhen = (booking: any) => {
    const days = sortedDays(booking)
    if (!days.length) return ''
    const [date, slots] = days[0]
    const rest = days.length - 1
    return `${formatDay(date, timezone.value)} · ${formatSlots(slots)}`
        + (rest ? ` +${rest} more ${rest === 1 ? 'day' : 'days'}` : '')
}
</script>

<template>
    <header class="grid grid-cols-[1fr_auto_1fr] items-center px-4 py-3">
        <Button size="icon-lg" @click="router.back()"
            class="justify-self-start bg-card text-card-foreground border border-border shadow-xs rounded-full">
            <ArrowLeft />
        </Button>
        <Title>Manage spot</Title>
        <div class="flex gap-2 justify-self-end">
            <Button size="icon-lg" @click="confirmOpen = true"
                class="bg-card text-card-foreground border border-border shadow-xs rounded-full">
                <Trash2 class="text-destructive" />
            </Button>
            <Button size="icon-lg" @click="routeToSpotEdit"
                class="bg-card text-card-foreground border border-border shadow-xs rounded-full">
                <Pencil />
            </Button>
        </div>
    </header>
    <div class="space-y-3 px-4 pb-2 overflow-y-auto no-scrollbar">
        <div class="flex h-40 gap-2 overflow-x-auto snap-x snap-mandatory no-scrollbar">
            <img v-for="key in data?.images" :key="key" :src="key"
                class="snap-center shrink-0 h-full w-auto only:w-full object-cover rounded-md" />
        </div>
        <div class="flex">
            <div class="flex-1">
                <Title size="lg">{{ data?.title }}</Title>
                <Text>
                    {{ data?.address?.formatted }}
                </Text>
            </div>
            <Money :cents="data?.pricePerHour ?? 0" suffix="/hr" size="lg" tone="primary" />
        </div>
        <Surface orientation="horizontal" class="justify-between">
            <div>
                <Text size="sm" tone="default">Listing is live</Text>
                <Text>
                    {{ live ? 'Drivers can book this now' : 'Hidden — bookings already made still stand' }}
                </Text>
            </div>
            <Switch :model-value="live" @update:model-value="toggleLive" />
        </Surface>
        <FieldError v-if="errors.length" :errors="errors" />
        <div class="grid grid-cols-3 gap-2">
            <Surface variant="elevated" class="text-center">
                <Money :cents="12800" size="md" class="justify-center" />
                <Text>Earned</Text>
            </Surface>
            <Surface variant="elevated" class="text-center">
                <Title>14</Title>
                <Text>Trips</Text>
            </Surface>
            <Surface variant="elevated" class="text-center">
                <Title class="flex justify-center items-center gap-1">
                    <Star :size="14" />
                    4.9
                </Title>
                <Text>Rating</Text>
            </Surface>
        </div>
        <div v-for="section in sections" :key="section.title" class="space-y-2">
            <SectionHeader>
                <Title>{{ section.title }}</Title>
                <template #action>
                    <Text tone="primary" weight="semibold" class="flex gap-1 items-center" @click="routeToSpotEdit">
                        <Pencil :size="16" />
                        Edit
                    </Text>
                </template>
            </SectionHeader>
            <div class="space-y-2">
                <Surface v-for="row in section.rows" :key="row.key" @click="toggle(row.key)">
                    <Text size="sm" tone="default">{{ row.label }}</Text>
                    <!-- Collapsed shows the first slot and how many are hidden; tapping
                         reveals the rest in place, because a day's hours are one thought. -->
                    <Text v-auto-animate>
                        <div v-for="slot in slotsShown(row)" :key="slot.start">
                            {{ formatSlots([slot]) }}
                        </div>
                        <div v-if="expanded !== row.key && row.slots.length > 1">
                            +{{ row.slots.length - 1 }} more
                        </div>
                    </Text>
                </Surface>
                <Text v-if="!section.rows.length">
                    {{ section.empty }}
                </Text>
            </div>
        </div>
        <div class="space-y-2">
            <SectionHeader>
                <Title>Upcoming bookings</Title>
                <template #action>
                    <Text v-if="upcoming.length > PREVIEW" @click="showAllBookings = !showAllBookings" tone="primary"
                        weight="semibold">
                        {{ showAllBookings ? 'Show less' : 'View all' }}
                    </Text>
                </template>
            </SectionHeader>
            <div v-auto-animate class="space-y-2 max-h-96 overflow-y-auto no-scrollbar">
                <Surface v-for="booking in visibleBookings" :key="booking.id" orientation="horizontal" class="gap-2">
                    <Avatar size="lg">
                        <AvatarImage v-if="booking?.renter?.profilePicture" :src="booking?.renter?.profilePicture" />
                        <AvatarFallback
                            :name="{ firstName: booking?.renter?.firstName ?? '', lastName: booking?.renter?.lastName ?? '' }" />
                    </Avatar>
                    <div class="flex-1">
                        <Text size="sm" tone="default">{{ booking?.renter?.firstName }} {{ booking?.renter?.lastName
                            }}
                        </Text>
                        <Text>{{ bookingWhen(booking) }}</Text>
                    </div>
                    <!-- Non-null for the host, who is a party to every booking on
                         their own listing. `?? 0` only covers the tick before the
                         query resolves. -->
                    <Money :cents="booking?.amount ?? 0" signed size="md" />
                </Surface>
                <Text v-if="!upcoming.length">
                    Nothing booked yet.
                </Text>
            </div>
        </div>
    </div>

    <Drawer v-model:open="confirmOpen">
        <DrawerContent @close-auto-focus.prevent
            class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
            <div class="m-4 space-y-4">
                <div>
                    <Title size="lg">Delete this listing?</Title>
                    <Text size="sm">
                        {{ data?.title }} comes off the market for good. Any booking it still
                        owes is cancelled and refunded. This can't be undone — to pause it instead,
                        turn off "Listing is live".
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
