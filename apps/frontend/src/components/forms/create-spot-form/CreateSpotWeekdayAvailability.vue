<script setup lang="ts">
import { z } from 'zod'
import { Trash2 } from '@lucide/vue'

import { Button } from '@/components/ui/button'
import {
    Select,
    SelectContent,
    SelectGroup,
    SelectItem,
    SelectLabel,
    SelectTrigger,
    SelectValue,
} from '@/components/ui/select'
import {
    Item,
    ItemActions,
    ItemContent,
    ItemTitle,
} from '@/components/ui/item'
import { ref } from 'vue'
import { CreateSpotRequest, TimeSlot, WeeklyAvailability } from '@/types/requests/CreateSpotRequest'
import Input from '@/components/ui/input/Input.vue'
import { useForm } from 'vee-validate'
import { Field as VeeField } from 'vee-validate'
import {
    Field,
    FieldError,
    FieldLabel,
} from '@/components/ui/field'
import { toTypedSchema } from '@vee-validate/zod'

const form = defineModel<CreateSpotRequest>('form', { required: true })

const day = ref<keyof WeeklyAvailability>("monday")
const showTimePicker = ref<boolean>(false)

const timeRegex = /^([01]\d|2[0-3]):[0-5]\d$/

const timeSlotSchema = z
    .object({
        from: z.string().regex(timeRegex, 'Invalid start time'),
        to: z.string().regex(timeRegex, 'Invalid end time')
    })
    .superRefine(({ from, to }, ctx) => {
        if (from > to) {
            ctx.addIssue({
                code: z.ZodIssueCode.custom,
                path: ['to'],
                message: 'End time must be after the start time.'
            })
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

        const existingSlots: TimeSlot[] = form.value.availability.weekly[day.value]

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
    form.value.availability.weekly[day.value].push({ start: data.from, end: data.to })
    form.value.availability.weekly[day.value].sort((a, b) => a.start.localeCompare(b.start))
    resetForm()
})
</script>

<template>
    <div class="space-y-2" v-auto-animate>
        <Select v-model="day">
            <SelectTrigger class="w-full">
                <SelectValue placeholder="Select a week day" />
            </SelectTrigger>
            <SelectContent>
                <SelectGroup>
                    <SelectLabel>Days</SelectLabel>
                    <SelectItem value="monday">
                        Monday
                    </SelectItem>
                    <SelectItem value="tuesday">
                        Tuesday
                    </SelectItem>
                    <SelectItem value="wednesday">
                        Wednesday
                    </SelectItem>
                    <SelectItem value="thursday">
                        Thursday
                    </SelectItem>
                    <SelectItem value="friday">
                        Friday
                    </SelectItem>
                    <SelectItem value="saturday">
                        Saturday
                    </SelectItem>
                    <SelectItem value="sunday">
                        Sunday
                    </SelectItem>
                </SelectGroup>
            </SelectContent>
        </Select>
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
        <Item v-for="(timeSlot, key) in form.availability.weekly[day]" variant="outline" :key="key">
            <ItemContent>
                <ItemTitle>{{ timeSlot.start }} - {{ timeSlot.end }}</ItemTitle>
            </ItemContent>
            <ItemActions>
                <Trash2 @click="() => form.availability.weekly[day].splice(key, 1)" class="text-red-400" />
            </ItemActions>
        </Item>
    </div>
</template>