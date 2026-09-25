<script setup lang="ts">
// The five navbar roots. Its opposite number is `DetailLayout`, for a screen you
// pushed onto one of them: that one centres a small title between a back arrow and
// the screen's actions, this one gives the page name the left edge and the size of
// a heading, because a tab root is somewhere you arrive rather than somewhere you
// drilled into.
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'
import { Title } from '@/components/base/text'

const props = defineProps<{
    /** The page name. Ignored when the `title` slot is filled. */
    title?: string
    /** Merged onto `<main>`: spacing is the screen's business, `px-4` is not. */
    class?: HTMLAttributes['class']
}>()
</script>

<template>
    <header class="flex shrink-0 items-center justify-between gap-2 px-4 py-4">
        <!-- For a screen whose heading is not a page name — Home greets the user by
             name, over a greeting line, at a size no other tab uses. -->
        <slot name="title">
            <Title as="h1" size="xl" weight="extrabold">{{ title }}</Title>
        </slot>
        <div v-if="$slots.actions" class="flex items-center gap-3">
            <slot name="actions"></slot>
        </div>
    </header>
    <!-- Deliberately not a scroll container, and deliberately no bottom padding.
         The header only stays put if something *below* it scrolls instead of the
         page, and which element that is differs per screen: Home scrolls everything
         under the header, My spots pins a stats grid and scrolls the listing list
         alone. So each view puts `min-h-0 flex-1 overflow-y-auto no-scrollbar pb-3`
         on the element it means, and this box only sets up the flex chain that lets
         that element shrink below its content.

         `gap-4` costs the single-child screens nothing — a gap only applies between
         children — and is what the screens with something pinned above their list
         were all passing anyway. `cn` merges, so a screen that wants another
         spacing passes it. -->
    <main :class="cn('flex min-h-0 flex-1 flex-col gap-4 px-4', props.class)">
        <slot></slot>
    </main>
</template>
