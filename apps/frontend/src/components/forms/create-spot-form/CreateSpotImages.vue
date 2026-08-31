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
import { ImagePlus, X } from '@lucide/vue'
import Button from '@/components/ui/button/Button.vue';

defineProps<{ imageErrors: string[] }>()

// One list for both kinds: a `string` is a photo already in R2 (its media URL), a
// `File` is one just picked and not uploaded yet. Editing a listing mixes them
// freely, and keeping them in separate models would mean two sets of previews, two
// remove buttons, and no single answer to "what order are the photos in".
const images = defineModel<(string | File)[]>('images', { required: true })

// Object URLs are derived from `images`; revoke the old batch so they don't leak.
// Stored images are already absolute URLs — nothing to build and nothing to revoke.
const previewUrls = ref<(string | undefined)[]>([])
const revokeAll = () =>
    previewUrls.value.forEach(u => u?.startsWith('blob:') && URL.revokeObjectURL(u))

watch(images, (items) => {
    revokeAll()
    previewUrls.value = items.map(i => typeof i === 'string' ? i : URL.createObjectURL(i))
}, { deep: true, immediate: true })

onScopeDispose(revokeAll)

const nameOf = (image: string | File) =>
    typeof image === 'string' ? image.split('/').pop() ?? 'Photo' : image.name

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
            <div v-auto-animate class="flex gap-2 overflow-x-auto snap-x snap-mandatory no-scrollbar">
                <div v-for="(image, index) in images" :key="index" class="relative shrink-0 snap-center">
                    <Dialog>
                        <DialogTrigger as-child>
                            <img :src="previewUrls[index]" :alt="nameOf(image)"
                                class="h-24 max-w-full rounded-md border object-contain" />
                        </DialogTrigger>
                        <DialogContent :show-close-button="false"
                            class="gap-0 border-0 bg-transparent p-0 ring-0 sm:max-w-2xl">
                            <!-- Screen readers still need a title; on screen the photo is the content. -->
                            <DialogTitle class="sr-only">{{ nameOf(image) }}</DialogTitle>
                            <DialogClose as-child>
                                <img :src="previewUrls[index]" :alt="nameOf(image)"
                                    class="max-h-[85vh] w-full rounded-lg object-contain" />
                            </DialogClose>
                        </DialogContent>
                    </Dialog>
                    <Button type="button" size="icon-xs" @click="images.splice(index, 1)"
                        class="absolute top-1 right-1 rounded-full bg-card text-card-foreground">
                        <X />
                    </Button>
                </div>
            </div>
        </div>
        <FieldError v-if="imageErrors.length" :errors="imageErrors" />
    </div>
</template>
