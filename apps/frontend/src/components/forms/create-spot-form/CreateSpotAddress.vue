<script setup lang="ts">
import { ref, watch } from 'vue'
import { useDebounceFn } from '@vueuse/core'
import { Field as VeeField } from 'vee-validate'
import {
    Field,
    FieldError,
    FieldGroup,
    FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
    Combobox,
    ComboboxAnchor,
    ComboboxInput,
    ComboboxList,
    ComboboxViewport,
    ComboboxItem,
    ComboboxEmpty,
} from '@/components/ui/combobox'
import { Text, Title } from '@/components/base/text'
import { suggestAddress } from '@/api/address'
import type { Address } from '@/types/domain/spot'
import { AcceptableValue } from 'reka-ui';

// setValues comes from the parent's useForm; picking a suggestion fills every
// address.* field in one shot.
const props = defineProps<{ setValues: (values: Record<string, any>, shouldValidate?: boolean) => void }>()

const term = ref('')
const items = ref<Address[]>([])
const open = ref(false)
// Set the input text on select without retriggering a search.
let suppress = false

const search = useDebounceFn(async (q: string) => {
    const query = q.trim()
    if (query.length < 3) {
        items.value = []
        open.value = false
        return
    }
    items.value = await suggestAddress(query)
    open.value = items.value.length > 0
}, 300)

watch(term, (q) => {
    if (suppress) {
        suppress = false
        return
    }
    search(q)
})

const onSelect = (value: AcceptableValue) => {
    const addr = value as Address
    props.setValues({
        address: {
            line1: addr.line1,
            line2: addr.line2 ?? undefined,
            city: addr.city,
            postalCode: addr.postalCode,
            region: addr.region ?? undefined,
            country: addr.country,
        },
    }, false)
    suppress = true
    term.value = addr.formatted
    items.value = []
    open.value = false
}
</script>

<template>
    <FieldGroup class="gap-4">
        <Title>Address</Title>
        <Field class="gap-1">
            <Combobox v-model:open="open" :ignore-filter="true" :reset-search-term-on-blur="false"
                @update:model-value="onSelect">
                <ComboboxAnchor class="bg-card rounded-md">
                    <ComboboxInput id="address-search" v-model="term" placeholder="Search your address" />
                </ComboboxAnchor>
                <ComboboxList>
                    <ComboboxViewport>
                        <ComboboxEmpty>No matches</ComboboxEmpty>
                        <ComboboxItem v-for="(item, i) in items" :key="i" :value="item">
                            {{ item.formatted }}
                        </ComboboxItem>
                    </ComboboxViewport>
                </ComboboxList>
            </Combobox>
            <Text as="p" weight="normal">
                Search powered by LocationIQ
            </Text>
        </Field>

        <VeeField v-slot="{ componentField, errors }" name="address.line1">
            <Field :data-invalid="!!errors.length" class="gap-1">
                <FieldLabel for="address.line1">
                    Line 1
                </FieldLabel>
                <Input id="address.line1" v-bind="componentField" placeholder="Line 1" autocomplete="off"
                    :aria-invalid="!!errors.length" class="bg-card" />
                <FieldError v-if="errors.length" :errors="errors" />
            </Field>
        </VeeField>

        <VeeField v-slot="{ componentField, errors }" name="address.line2">
            <Field :data-invalid="!!errors.length" class="gap-1">
                <FieldLabel for="address.line2">
                    Line 2
                </FieldLabel>
                <Input id="address.line2" v-bind="componentField" placeholder="Line 2" autocomplete="off"
                    :aria-invalid="!!errors.length" class="bg-card" />
                <FieldError v-if="errors.length" :errors="errors" />
            </Field>
        </VeeField>

        <div class="flex gap-4">
            <VeeField v-slot="{ componentField, errors }" name="address.city">
                <Field :data-invalid="!!errors.length" class="gap-1">
                    <FieldLabel for="address.city">
                        City
                    </FieldLabel>
                    <Input id="address.city" v-bind="componentField" placeholder="City" autocomplete="off"
                        :aria-invalid="!!errors.length" class="bg-card" />
                    <FieldError v-if="errors.length" :errors="errors" />
                </Field>
            </VeeField>

            <VeeField v-slot="{ componentField, errors }" name="address.postalCode">
                <Field :data-invalid="!!errors.length" class="gap-1">
                    <FieldLabel for="address.postalCode">
                        Postal Code
                    </FieldLabel>
                    <Input id="address.postalCode" v-bind="componentField" placeholder="Postal Code" autocomplete="off"
                        :aria-invalid="!!errors.length" class="bg-card" />
                    <FieldError v-if="errors.length" :errors="errors" />
                </Field>
            </VeeField>
        </div>

        <div class="flex gap-4">
            <VeeField v-slot="{ componentField, errors }" name="address.region">
                <Field :data-invalid="!!errors.length" class="gap-1">
                    <FieldLabel for="address.region">
                        Region
                    </FieldLabel>
                    <Input id="address.region" v-bind="componentField" placeholder="Region" autocomplete="off"
                        :aria-invalid="!!errors.length" class="bg-card" />
                    <FieldError v-if="errors.length" :errors="errors" />
                </Field>
            </VeeField>

            <VeeField v-slot="{ componentField, errors }" name="address.country">
                <Field :data-invalid="!!errors.length" class="gap-1">
                    <FieldLabel for="address.country">
                        Country
                    </FieldLabel>
                    <Input id="address.country" v-bind="componentField" placeholder="Country" autocomplete="off"
                        :aria-invalid="!!errors.length" class="bg-card" />
                    <FieldError v-if="errors.length" :errors="errors" />
                </Field>
            </VeeField>
        </div>
    </FieldGroup>
</template>
