<script setup lang="ts">
import { Field as VeeField } from 'vee-validate'
import {
    Field,
    FieldDescription,
    FieldError,
    FieldGroup,
    FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
    InputGroup,
    InputGroupAddon,
    InputGroupText,
    InputGroupTextarea,
} from '@/components/ui/input-group'
import { Title } from '@/components/base/text'
</script>

<template>
    <FieldGroup class="gap-4">
        <Title>Basic details</Title>
        <VeeField v-slot="{ componentField, errors }" name="title">
            <Field :data-invalid="!!errors.length" class="gap-1">
                <FieldLabel for="title">
                    Title
                </FieldLabel>
                <Input id="title" v-bind="componentField" placeholder="Title" autocomplete="off"
                    :aria-invalid="!!errors.length" class="bg-card" />
                <FieldError v-if="errors.length" :errors="errors" />
            </Field>
        </VeeField>

        <VeeField v-slot="{ field, componentField, errors }" name="description">
            <Field :data-invalid="!!errors.length" class="gap-1">
                <FieldLabel for="description">
                    Description
                </FieldLabel>
                <InputGroup class="bg-card">
                    <InputGroupTextarea id="description" v-bind="componentField" placeholder="Description" :rows="4"
                        class="min-h-24 resize-none" :aria-invalid="!!errors.length" />
                    <InputGroupAddon align="block-end">
                        <InputGroupText class="tabular-nums">
                            {{ field.value?.length || 0 }}/100 characters
                        </InputGroupText>
                    </InputGroupAddon>
                </InputGroup>
                <FieldDescription>
                    Custom parking instructions etc
                </FieldDescription>
                <FieldError v-if="errors.length" :errors="errors" />
            </Field>
        </VeeField>

        <VeeField v-slot="{ componentField, errors }" name="pricePerHour">
            <Field :data-invalid="!!errors.length" class="gap-1">
                <FieldLabel for="pricePerHour">
                    Price per hour
                </FieldLabel>
                <Input type="number" id="pricePerHour" v-bind="componentField" placeholder="Price per hour"
                    autocomplete="off" :aria-invalid="!!errors.length" class="bg-card" />
                <FieldError v-if="errors.length" :errors="errors" />
            </Field>
        </VeeField>
    </FieldGroup>
</template>
