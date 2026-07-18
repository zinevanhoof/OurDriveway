<script setup lang="ts">
import { cn } from '@/lib/utils';
import { AnimatePresence, motion } from 'motion-v';
import { computed, inject } from 'vue';

defineOptions({
    inheritAttrs: false,
})

const { direction, initial, animate, exit } = defineProps<{
    direction: 'top' | 'bottom',
    initial: Record<string, any>,
    animate: Record<string, any>,
    exit: Record<string, any>,
}>()
const open = defineModel<boolean>()

const safeTop = inject<number>('safeTop')
const safeBottom = inject<number>('safeBottom')

const which = computed(() => direction === "top" ? animate.y + safeTop! : animate.y - safeBottom!)

const mergedAnimate = computed(() => ({
    ...animate,
    y: which.value,
}))
</script>

<template>
    <AnimatePresence>
        <motion.div v-if="open" class="fixed inset-0 bg-black/40" :initial="{ opacity: 0 }" :animate="{ opacity: 1 }"
            :exit="{ opacity: 0 }" @click="open = false" />
        <motion.div v-if="open" :class="cn('fixed bottom-0 left-0 right-0 bg-white rounded-xl mx-4', $attrs.class)"
            :initial="initial" :animate="mergedAnimate" :exit="exit"
            :transition="{ type: 'spring', damping: 25, stiffness: 300 }">
            <slot></slot>
        </motion.div>
    </AnimatePresence>
</template>