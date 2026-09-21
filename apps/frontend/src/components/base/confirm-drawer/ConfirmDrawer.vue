<script setup lang="ts">
// "Are you sure?" as a bottom sheet: a heading, a sentence of consequence, and two
// buttons — the destructive one first, the way out second.
//
// This was the same twenty lines in four places (log out, delete a listing twice, cancel
// a booking), identical down to the `mb-15` that clears the navbar. The copies had
// already drifted apart on nothing that mattered, which is the argument for one file.
//
// The body is a slot, not a prop: every caller interpolates something — a spot title, a
// date — and two of them branch on state.
import { Drawer, DrawerContent } from '@/components/ui/drawer'
import { Text, Title } from '@/components/base/text'
import Button from '@/components/ui/button/Button.vue'

const open = defineModel<boolean>('open')

const {
    confirmLabel,
    cancelLabel = 'Cancel',
    pending = false,
    pendingLabel,
} = defineProps<{
    title: string
    confirmLabel: string
    cancelLabel?: string
    /** Disables both buttons and swaps the confirm label while the action is in flight. */
    pending?: boolean
    pendingLabel?: string
}>()

const emit = defineEmits<{ confirm: [] }>()
</script>

<template>
    <!-- `mb-15` clears the navbar, which is z-60 over the drawer's z-50 and would
         otherwise sit on top of the buttons. -->
    <Drawer v-model:open="open">
        <DrawerContent @close-auto-focus.prevent class="data-[vaul-drawer-direction=bottom]:mb-15">
            <div class="m-4 space-y-4">
                <div>
                    <Title size="lg">{{ title }}</Title>
                    <Text size="sm">
                        <slot />
                    </Text>
                </div>
                <div class="space-y-2">
                    <Button variant="destructive" class="w-full h-11 font-bold" :disabled="pending"
                        @click="emit('confirm')">
                        {{ pending ? (pendingLabel ?? confirmLabel) : confirmLabel }}
                    </Button>
                    <Button variant="outline" class="w-full h-11 font-bold" :disabled="pending" @click="open = false">
                        {{ cancelLabel }}
                    </Button>
                </div>
            </div>
        </DrawerContent>
    </Drawer>
</template>
