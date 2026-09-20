<script setup lang="ts">
import { X } from '@lucide/vue';
import { AnimatePresence, motion } from 'motion-v';
import Button from './ui/button/Button.vue';
import { Text, Title } from '@/components/base/text';

const { title, description, showAction = true } = defineProps<{
    title: string
    description?: string
    /**
     * Whether the `action` slot is being offered right now — a save button on a form
     * nobody has touched is an offer with nothing behind it.
     *
     * Defaults to `true`, so a screen that just wants a floating button passes the slot
     * and nothing else. Defaulting to `false` would make a forgotten prop look exactly
     * like a broken button.
     *
     * Ignored when there is no `action` slot.
     */
    showAction?: boolean
}>()

const emit = defineEmits(['close'])
</script>

<template>
    <header class="flex items-center gap-2 px-4 py-3.5 bg-card border-b border-border">
        <Button size="icon" @click="emit('close')" class="bg-muted text-foreground">
            <X />
        </Button>
        <div>
            <Title weight="extrabold">{{ title }}</Title>
            <Text v-if="description" weight="semibold">{{ description }}</Text>
        </div>
    </header>
    <!-- Page padding lives here and not in the view, unlike every other screen: this
         `<main>` is the scroll container, and padding outside it would not scroll.

         Bottom is the app-wide `pb-3`, plus the floating action's height when there is
         one — the bar overlays this box rather than taking space from it. `pb-28` (7rem)
         is exactly that: 0.75rem + the bar's pt-10 + h-11 button + pb-4 (6.25rem).
         A ternary, not `pb-3` plus a conditional `pb-28`: two padding utilities on one
         element are decided by stylesheet order, not by which was written last. -->
    <main class="space-y-4 flex-1 px-4 pt-4 bg-background overflow-y-auto no-scrollbar"
        :class="$slots.action ? 'pb-28' : 'pb-3'">
        <slot name="main"></slot>
    </main>
    <!-- Only when someone fills it. An always-rendered footer is a padded, bordered
         strip of nothing on every screen whose action floats instead. -->
    <footer v-if="$slots.footer" class="px-4 pb-4 pt-3 bg-card border-t border-border">
        <slot name="footer"></slot>
    </footer>
    <!-- The floating action: a button that arrives when there is something to do with it.
         `AnimatePresence` is what gives it an exit — undoing the change puts it away the
         same way it came.

         **The containing block comes from the parent.** This component has no root
         element of its own, so `absolute` resolves against the nearest positioned
         ancestor: `MobileLayout`'s `<main class="relative">` for a routed screen, and
         vaul's `fixed` drawer content for the booking form. Both put `bottom-0` where it
         belongs — above the navbar, or at the bottom of the sheet. A third kind of parent
         has to be positioned too.

         `pointer-events-none` on the fade and `auto` on the button's wrapper: the scrim
         spans the width and would otherwise swallow taps meant for the content under it.

         **`key` is load-bearing.** `AnimatePresence` renders a Vue `<TransitionGroup>`,
         and TransitionGroup attaches its hooks only to *keyed* children — an unkeyed one
         gets a dev warning and nothing else. So without this, `onEnter` never fires,
         motion applies `initial` at mount and leaves it there, and the button is mounted
         at `opacity: 0` forever: present in the DOM, invisible on every screen. -->
    <AnimatePresence>
        <motion.div v-if="$slots.action && showAction" key="action"
            class="pointer-events-none absolute inset-x-0 bottom-0 px-4 pb-4 pt-10 bg-linear-to-t from-background via-background/95 to-transparent"
            :initial="{ opacity: 0, y: 32 }" :animate="{ opacity: 1, y: 0 }" :exit="{ opacity: 0, y: 32 }"
            :transition="{ type: 'spring', stiffness: 500, damping: 35 }">
            <div class="pointer-events-auto">
                <slot name="action"></slot>
            </div>
        </motion.div>
    </AnimatePresence>
</template>
