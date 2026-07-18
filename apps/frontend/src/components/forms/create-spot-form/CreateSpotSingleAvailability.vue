<script setup lang="ts">
import { z } from 'zod'
import { Trash2 } from '@lucide/vue'

import { Button } from '@/components/ui/button'
import {
    Item,
    ItemActions,
    ItemContent,
    ItemTitle,
} from '@/components/ui/item'
import { inject, ref } from 'vue'
import { CreateSpotRequest, TimeSlot } from '@/types/requests/CreateSpotRequest'
import Input from '@/components/ui/input/Input.vue'
import { useForm } from 'vee-validate'
import { Field as VeeField } from 'vee-validate'
import {
    Field,
    FieldError,
    FieldLabel,
} from '@/components/ui/field'
import { toTypedSchema } from '@vee-validate/zod'

import type { DateValue } from '@internationalized/date'
import { DateFormatter, getLocalTimeZone, today } from '@internationalized/date'

import { CalendarIcon } from '@lucide/vue'
import { cn } from '@/lib/utils'
import { Calendar } from '@/components/ui/calendar'
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from '@/components/ui/popover'

const defaultPlaceholder = today(getLocalTimeZone())
const minDate = defaultPlaceholder

const df = new DateFormatter('en-US', {
    dateStyle: 'long',
})

const safeBottom = inject<number>('safeBottom')

const form = defineModel<CreateSpotRequest>('form', { required: true })

const date = ref<DateValue>()
const showTimePicker = ref<boolean>(false)

const timeRegex = /^([01]\d|2[0-3]):[0-5]\d$/

const timeSlotSchema = z
    .object({
        from: z.string().regex(timeRegex, 'Invalid start time'),
        to: z.string().regex(timeRegex, 'Invalid end time')
    })
    .superRefine(({ from, to }, ctx) => {
        if (date.value === undefined) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['sharedError'],
                message: 'No date selected.',
            })
            return
        }

        const [, fromMinutes] = from.split(':').map(Number)
        const [, toMinutes] = to.split(':').map(Number)

        if (fromMinutes % 30 !== 0) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['from'],
                message: 'From time must be in 30 minute increments.'
            })
        }

        if (toMinutes % 30 !== 0) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['to'],
                message: 'To time must be in 30 minute increments.'
            })
        }

        if (from > to) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['to'],
                message: 'End time must be after the start time.'
            })
        }

        form.value.availability.single[date.value.toString()] ??= []
        const existingSlots: TimeSlot[] = form.value.availability.single[date.value.toString()]

        const overlaps = existingSlots.some(slot =>
            from < slot.end && to > slot.start
        )

        if (overlaps) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['sharedError'],
                message: 'This time slot overlaps with an existing slot.',
            })
        }

        const duplicate = existingSlots.some(slot =>
            slot.start === from && slot.end === to
        )

        if (duplicate) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['sharedError'],
                message: 'This time slot already exists.',
            })
        }
    })

const { handleSubmit, resetForm } = useForm({
    validationSchema: toTypedSchema(timeSlotSchema)
})

const saveTimeSlot = handleSubmit((data: any) => {
    if (date.value === undefined)
        return;

    form.value.availability.single[date.value.toString()] ??= []
    form.value.availability.single[date.value.toString()].push({
        start: data.from,
        end: data.to
    })
    form.value.availability.single[date.value.toString()].sort((a, b) => a.start.localeCompare(b.start))
    resetForm()
})
</script>

<template>
    <div class="space-y-2" v-auto-animate>
        <Popover v-slot="{ close }">
            <PopoverTrigger as-child>
                <Button variant="outline" :class="cn('w-full justify-start', !date && 'text-muted-foreground')">
                    <CalendarIcon />
                    {{ date ? df.format(date.toDate(getLocalTimeZone())) : "Pick a date" }}
                </Button>
            </PopoverTrigger>
            <PopoverContent class="w-auto p-0" align="start" :collision-padding="{ bottom: safeBottom }">
                <Calendar v-model="date" :default-placeholder="defaultPlaceholder" :min-value="minDate"
                    layout="month-and-year" initial-focus @update:model-value="close" />
            </PopoverContent>
        </Popover>
        <Button v-if="!showTimePicker" type="button" @click="showTimePicker = true" class="w-full">Add time
            slot</Button>
        <Item v-if="showTimePicker" variant="outline">
            <div class="flex w-full gap-4">
                <VeeField v-slot="{ componentField, errors }" name="from">
                    <Field :data-invalid="!!errors.length">
                        <FieldLabel for="from">
                            From
                        </FieldLabel>
                        <Input type="time" id="from" v-bind="componentField" placeholder="From" autocomplete="off"
                            :aria-invalid="!!errors.length" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
                <VeeField v-slot="{ componentField, errors }" name="to">
                    <Field :data-invalid="!!errors.length">
                        <FieldLabel for="to">
                            To
                        </FieldLabel>
                        <Input type="time" id="to" v-bind="componentField" placeholder="From" autocomplete="off"
                            :aria-invalid="!!errors.length" />
                        <FieldError v-if="errors.length" :errors="errors" />
                    </Field>
                </VeeField>
            </div>
            <VeeField v-slot="{ errors }" name="sharedError">
                <FieldError v-if="errors.length" :errors="errors" />
            </VeeField>
            <div class="flex w-full gap-4">
                <Button type="button" @click="showTimePicker = false" variant="destructive"
                    class="flex-1">Close</Button>
                <Button type="button" @click="saveTimeSlot" class="flex-1">Save</Button>
            </div>
        </Item>
        <Item v-for="(timeSlot, key) in form.availability.single[date?.toString()!]" variant="outline" :key="key">
            <ItemContent>
                <ItemTitle>{{ timeSlot.start }} - {{ timeSlot.end }}</ItemTitle>
            </ItemContent>
            <ItemActions>
                <Trash2 @click="() => form.availability.single[date?.toString()!].splice(key, 1)" color="red" />
            </ItemActions>
        </Item>
    </div>
</template>