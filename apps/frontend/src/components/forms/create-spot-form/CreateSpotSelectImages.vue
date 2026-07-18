<script setup lang="ts">
import Input from '@/components/ui/input/Input.vue'
import { ref } from 'vue'

import {
    Dialog,
    DialogContent,
    DialogHeader,
    DialogTitle,
    DialogTrigger,
} from '@/components/ui/dialog'

type Image = {
    name: string
    url: string
}

const previewUrls = ref<Image[]>([])

const test = async (event: Event) => {
    const input = event.target as HTMLInputElement

    if (input.files === null)
        return

    for (let i = 0; i < input.files.length; i++) {
        const file = input.files[i]
        const url = URL.createObjectURL(file)
        images.value.push(file)
        previewUrls.value[i] = {
            name: file.name,
            url
        }
    }
}

const images = defineModel<File[]>('images', { required: true })
</script>

<template>
    <div class="space-y-2">
        <div class="flex gap-2 overflow-x-auto">
            <Dialog v-for="(image, index) in previewUrls">
                <DialogTrigger as-child>
                    <img :src="image.url" :key="index" class="h-24 max-w-full rounded-md border object-contain" />
                </DialogTrigger>
                <DialogContent>
                    <DialogHeader>
                        <DialogTitle>{{ image.name }}</DialogTitle>
                    </DialogHeader>
                    <img :src="image.url" :key="index" class="rounded-md border" />
                </DialogContent>
            </Dialog>
        </div>
        <Input type="file" accept="image/*" multiple @change="test" />
    </div>
</template>