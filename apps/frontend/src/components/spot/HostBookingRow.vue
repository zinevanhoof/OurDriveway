<script setup lang="ts">
// One booking on a host's own spot: who, when, which car, how much.
//
// Shared by the manage screen's preview and the bookings screen so the two cannot drift.
// Attributes fall through to the `Surface`, so a caller that opens a drawer passes
// `interactive` and `@click`, and the preview passes neither.
import { Car } from '@lucide/vue'

import Avatar from '@/components/ui/avatar/Avatar.vue'
import AvatarImage from '@/components/ui/avatar/AvatarImage.vue'
import AvatarFallback from '@/components/ui/avatar/AvatarFallback.vue'
import { Badge } from '@/components/ui/badge'
import { Surface } from '@/components/base/surface'
import { Text } from '@/components/base/text'
import { Money } from '@/components/base/money'

import type { HostBookingListItemResponse } from '@/types/responses/view/HostBookingListItemResponse'
import { formatDay, formatSlots, sortedDays } from '@/lib/bookingDates'

const { booking, timezone } = defineProps<{
    booking: HostBookingListItemResponse
    timezone?: string
}>()

/** "Mon, Aug 3 · 09:00–10:00", plus a count when the booking spans more days. */
const when = () => {
    const days = sortedDays(booking)
    if (!days.length) return ''
    const [date, slots] = days[0]
    const rest = days.length - 1
    return `${formatDay(date, timezone)} · ${formatSlots(slots)}`
        + (rest ? ` +${rest} more ${rest === 1 ? 'day' : 'days'}` : '')
}

/** Only the rows that are not a plain paid booking say anything. */
const badge = () => {
    switch (booking.status) {
        case 'reserved': return { label: 'Awaiting payment', variant: 'secondary' } as const
        case 'cancelled': return { label: 'Cancelled', variant: 'destructive' } as const
        case 'released': return { label: 'Expired', variant: 'outline' } as const
        default: return null
    }
}

// A cancelled or expired booking earned nothing, so its amount is not a credit.
const earned = () => booking.status === 'confirmed' || booking.status === 'reserved'
</script>

<template>
    <Surface orientation="horizontal" class="gap-2">
        <Avatar size="lg">
            <AvatarImage v-if="booking.renter?.profilePicture" :src="booking.renter.profilePicture" />
            <AvatarFallback :name="{
                firstName: booking.renter?.firstName ?? '',
                lastName: booking.renter?.lastName ?? '',
            }" />
        </Avatar>
        <div class="flex-1 min-w-0">
            <Text size="sm" tone="default" class="flex items-center gap-1.5">
                {{ booking.renter ? `${booking.renter.firstName} ${booking.renter.lastName}` : 'A renter' }}
                <Badge v-if="badge()" :variant="badge()!.variant">
                    {{ badge()!.label }}
                </Badge>
            </Text>
            <Text class="truncate">{{ when() }}</Text>
            <Text class="flex items-center gap-1">
                <Car :size="14" />
                {{ booking.licensePlate }}
            </Text>
        </div>
        <Money :cents="booking.amount" :signed="earned()" size="md" />
    </Surface>
</template>
