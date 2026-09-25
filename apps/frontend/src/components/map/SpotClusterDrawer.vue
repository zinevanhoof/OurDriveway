<script setup lang="ts">
// Tapping a pin that stands for several spots at one location: pick which one.
//
// The drawer's awkward parts are all here rather than in the map, but the *state* stays
// there — `open` and the member list are separate on purpose, so the sheet keeps its
// height through the close animation, and `animation-end` is what tells the map it is
// finally safe to drop the members. See the notes on `clusterOpen` in LocationMap.
import { Drawer, DrawerContent } from '@/components/ui/drawer'
import { Surface } from '@/components/base/surface'
import { Title } from '@/components/base/text'
import { Money } from '@/components/base/money'
import type { MapSpot } from '@/lib/mapPins'

const open = defineModel<boolean>('open')

defineProps<{ spots?: MapSpot[] }>()

const emit = defineEmits<{ select: [id: string]; animationEnd: [open: boolean] }>()
</script>

<template>
    <Drawer v-model:open="open" :modal="false" @animation-end="emit('animationEnd', $event)">
        <!-- No overlay, and outside pointer-downs are left alone: that event beats the
             marker's click, so letting it dismiss would close and reopen the sheet on
             every pin-to-pin tap. The canvas click handler closes it instead. -->
        <!-- The list scrolls in the inner box, not the sheet, which is how every other
             drawer here is built. The sheet used to be its own scroll container — vaul
             prefers that for drag-to-close, since it only takes a downward drag when the
             scroll container it finds is the dialog itself. The price was vaul's
             `height: 200%` `::after`, the strip of sheet-coloured background that sits
             below the sheet: inside a scroll container that pseudo becomes scrollable
             content, two screens of empty popover under a three-row list, so it had to be
             hidden. Hiding it is what left the map showing through the gap when the sheet
             is dragged up past its resting place. Scrolling the inner box keeps the
             `::after` out of any scroller, so it can do its job. -->
        <DrawerContent @close-auto-focus.prevent :overlay="false" @pointer-down-outside.prevent
            class="data-[vaul-drawer-direction=bottom]:mb-15">
            <div class="m-4 space-y-3 overflow-y-auto no-scrollbar">
                <Title size="lg">{{ spots?.length }} spots here</Title>
                <div class="space-y-2">
                    <Surface v-for="s in spots" :key="s.id" @click="emit('select', s.id)" as="button" variant="none"
                        orientation="horizontal" class="w-full justify-between gap-4 border border-border text-left">
                        <Title as="span" weight="semibold" class="truncate">{{ s.title }}</Title>
                        <Money :cents="s.price" suffix="/hr" size="md" weight="extrabold" tone="primary"
                            class="shrink-0" />
                    </Surface>
                </div>
            </div>
        </DrawerContent>
    </Drawer>
</template>
