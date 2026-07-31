<script setup lang="ts">
import { onScopeDispose, ref, watch } from 'vue'
import { FieldError } from '@/components/ui/field'
import {
    Dialog,
    DialogClose,
    DialogContent,
    DialogTitle,
    DialogTrigger,
} from '@/components/ui/dialog'
import { ImagePlus } from '@lucide/vue'

defineProps<{ imageErrors: string[] }>()

const images = defineModel<File[]>('images', { required: true })

// Object URLs are derived from `images`; revoke the old batch so they don't leak.
const previewUrls = ref<string[]>([])
const revokeAll = () => previewUrls.value.forEach(u => URL.revokeObjectURL(u))

watch(images, (files) => {
    revokeAll()
    previewUrls.value = files.map(f => URL.createObjectURL(f))
}, { deep: true })

onScopeDispose(revokeAll)

const onSelectImages = (event: Event) => {
    const input = event.target as HTMLInputElement

    if (input.files === null)
        return

    images.value.push(...input.files)
}
</script>

<template>
    <div class="space-y-2">
        <div class="font-bold">Photos</div>
        <div class="flex gap-2">
            <label
                class="relative flex shrink-0 flex-col justify-center items-center w-24 h-24 text-xs text-accent-foreground font-semibold bg-card-2 rounded-lg border border-dashed border-border">
                <ImagePlus />
                Add photo
                <input type="file" accept="image/*" multiple class="sr-only" @change="onSelectImages" />
            </label>
            <div class="flex gap-2 overflow-x-auto snap-x snap-mandatory no-scrollbar">
                <Dialog v-for="(image, index) in images" :key="index">
                    <DialogTrigger as-child>
                        <img :src="previewUrls[index]" :alt="image.name"
                            class="h-24 max-w-full shrink-0 snap-center rounded-md border object-contain" />
                    </DialogTrigger>
                    <DialogContent :show-close-button="false"
                        class="gap-0 border-0 bg-transparent p-0 ring-0 sm:max-w-2xl">
                        <!-- Screen readers still need a title; on screen the photo is the content. -->
                        <DialogTitle class="sr-only">{{ image.name }}</DialogTitle>
                        <DialogClose as-child>
                            <img :src="previewUrls[index]" :alt="image.name"
                                class="max-h-[85vh] w-full rounded-lg object-contain" />
                        </DialogClose>
                    </DialogContent>
                </Dialog>
            </div>
        </div>
        <FieldError v-if="imageErrors.length" :errors="imageErrors" />
    </div>
</template>
