<script setup lang="ts">
import { computed, inject, ref } from 'vue'
import { FieldError } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import Button from '@/components/ui/button/Button.vue';
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'
import { Calendar } from '@/components/ui/calendar'
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from '@/components/ui/popover'
import { ArrowRight, CalendarIcon, Plus, X } from '@lucide/vue'
import {
    DateFormatter,
    getLocalTimeZone,
    parseDate,
    today,
    type DateValue,
} from '@internationalized/date'
import { cn } from '@/lib/utils'
import type { Availability, TimeSlot, WeeklyAvailability } from '@/types/domain/spot'

const availability = defineModel<Availability>('availability', { required: true })
const slotErrors = defineModel<string[]>('slotErrors', { required: true })

const safeBottom = inject<number>('safeBottom')
const df = new DateFormatter('en-US', { dateStyle: 'medium' })
const minDate = today(getLocalTimeZone())

const WEEKDAYS: (keyof WeeklyAvailability)[] =
    ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday']

const mode = ref<'recurring' | 'one-off'>('recurring')
const weekday = ref<keyof WeeklyAvailability>('monday')
const date = ref<DateValue>()
const start = ref('')
const end = ref('')

const addSlot = () => {
    slotErrors.value = []

    if (mode.value === 'one-off' && !date.value) {
        slotErrors.value.push('Pick a date first.')
        return
    }
    if (!start.value || !end.value) {
        slotErrors.value.push('Enter a start and an end time.')
        return
    }
    if (start.value >= end.value) {
        slotErrors.value.push('End time must be after the start time.')
        return
    }

    const slots: TimeSlot[] = mode.value === 'recurring'
        ? availability.value.weekly[weekday.value]
        : (availability.value.single[date.value!.toString()] ??= [])

    // Overlap covers exact duplicates too.
    if (slots.some(s => start.value < s.end && end.value > s.start)) {
        slotErrors.value.push('This overlaps a time slot you already added.')
        return
    }

    slots.push({ start: start.value, end: end.value })
    slots.sort((a, b) => a.start.localeCompare(b.start))
    start.value = ''
    end.value = ''
}

const groups = computed(() => [
    {
        title: 'Recurring',
        rows: WEEKDAYS
            .filter(d => availability.value.weekly[d].length)
            .map(d => ({
                label: d[0].toUpperCase() + d.slice(1),
                slots: availability.value.weekly[d],
            })),
    },
    {
        title: 'One-off',
        rows: Object.keys(availability.value.single)
            .filter(d => availability.value.single[d].length)
            .sort()
            .map(d => ({
                label: df.format(parseDate(d).toDate(getLocalTimeZone())),
                slots: availability.value.single[d],
            })),
    },
].filter(g => g.rows.length))
</script>

<template>
    <div class="space-y-2">
        <div>
            <div class="font-bold">Availability</div>
            <div class="text-xs text-muted-foreground font-medium">Add the times your spot is free —
                repeating
                every week, or on specific one-off dates.</div>
        </div>
        <Tabs v-model="mode">
            <TabsList class="w-full group-data-horizontal/tabs:h-10">
                <TabsTrigger value="recurring" class="font-bold">
                    Recurring days
                </TabsTrigger>
                <TabsTrigger value="one-off" class="font-bold">
                    Specific dates
                </TabsTrigger>
            </TabsList>
            <TabsContent value="recurring">
                <div class="flex gap-1">
                    <Button v-for="day in WEEKDAYS" :key="day" type="button"
                        :variant="weekday === day ? 'default' : 'outline'" class="flex-1 capitalize"
                        @click="weekday = day">
                        {{ day.slice(0, 3) }}
                    </Button>
                </div>
            </TabsContent>
            <TabsContent value="one-off">
                <Popover v-slot="{ close }">
                    <PopoverTrigger as-child>
                        <Button type="button" variant="outline"
                            :class="cn('w-full justify-start', !date && 'text-muted-foreground')">
                            <CalendarIcon />
                            {{ date ? df.format(date.toDate(getLocalTimeZone())) : 'Pick a date' }}
                        </Button>
                    </PopoverTrigger>
                    <PopoverContent class="w-auto p-0" align="start" :collision-padding="{ bottom: safeBottom }">
                        <Calendar v-model="date" :min-value="minDate" layout="month-and-year" initial-focus
                            @update:model-value="close" />
                    </PopoverContent>
                </Popover>
            </TabsContent>
        </Tabs>

        <!-- Same picker in both modes; the active tab only decides where the slot lands. -->
        <div class="flex items-center gap-1">
            <Input type="time" v-model="start" class="flex-1" />
            <ArrowRight class="text-muted-foreground size-4" />
            <Input type="time" v-model="end" class="flex-1" />
            <Button size="icon" type="button" @click="addSlot">
                <Plus />
            </Button>
        </div>
        <FieldError v-if="slotErrors.length" :errors="slotErrors" />

        <div class="space-y-2" v-auto-animate>
            <div v-for="group in groups" :key="group.title" class="space-y-2">
                <div class="text-xs font-bold uppercase text-muted-foreground">{{ group.title }}</div>
                <div v-for="row in group.rows" :key="row.label"
                    class="space-y-2 px-3 py-3.25 bg-card border border-border rounded-md">
                    <div class="text-sm font-bold">{{ row.label }}</div>
                    <div class="flex flex-wrap gap-1">
                        <div v-for="(slot, i) in row.slots" :key="slot.start"
                            class="flex items-center text-xs gap-1 rounded-md bg-muted px-2 py-0.5 font-semibold">
                            {{ slot.start }} - {{ slot.end }}
                            <button type="button" @click="row.slots.splice(i, 1)">
                                <X class="size-3" />
                            </button>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </div>
</template>
