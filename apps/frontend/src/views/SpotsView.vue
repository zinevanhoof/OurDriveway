<script setup lang="ts">
import { computed, ref } from 'vue';
import { watch } from 'vue'
import { useAddSpot } from '@/composables/useAddSpot'
import AnimatedSheet from '@/components/AnimatedSheet.vue';
import CreateSpotForm from '@/components/forms/create-spot-form/CreateSpotForm.vue';
import { CreateSpotRequest } from '@/types/requests/CreateSpotRequest';
import { createSpot } from '@/api/userApi';
import { useServiceQuery } from '@/composables/useServiceQuery';

import {
    Item,
    ItemContent,
    ItemTitle,
} from '@/components/ui/item'
import ItemDescription from '@/components/ui/item/ItemDescription.vue';
import { SPOTS_OWNED } from '@/api/graphql/spot';
import { useAuthStore } from '@/stores/auth';
import { useRouter } from 'vue-router';

const router = useRouter()

const { addSpotClicked, reset } = useAddSpot()

watch(addSpotClicked, (val) => {
    if (val) {
        openSheet.value = true
        reset()
    }
})

type CreateSpotFormSubmit = {
    form: CreateSpotRequest,
    images: File[]
}

const openSheet = ref(false)
const loading = ref(false)
const formRef = ref()

const auth = useAuthStore()

// Declarative subscription: fetches on mount, exposes reactive `data`.
// Must be set up here in setup(), not inside an event handler.
const { data, executeQuery } = useServiceQuery("spot", {
    query: SPOTS_OWNED,
    variables: computed(() => ({ id: auth.user?.id })),
})

const onSubmit = async (submit: CreateSpotFormSubmit) => {
    loading.value = true
    try {
        const formData = new FormData()
        formData.append('data', JSON.stringify(submit.form))
        for (const image of submit.images) formData.append('images', image)
        const response = await createSpot(formData)
        if (!response.ok) {
            const body = await response.json().catch(() => ({}))
            formRef.value?.showServerErrors(body) // jump to the offending step, keep sheet open
            return
        }
        openSheet.value = false
        executeQuery({ requestPolicy: 'network-only' }) // refetch list with the new spot
    } finally {
        loading.value = false
    }
}
</script>

<template>
    <div class="space-y-2 mt-2 mx-2">
        <Item @click="() => router.push({ name: 'spot', params: { id: spot.id } })" v-for="spot in data?.spots"
            variant="outline" :key="spot.id">
            <ItemContent>
                <ItemTitle>{{ spot.title }}</ItemTitle>
                <ItemDescription>{{ spot.address.formatted }}</ItemDescription>
            </ItemContent>
        </Item>
    </div>
    <AnimatedSheet v-model="openSheet" direction="bottom" :initial="{ y: '100%' }" :animate="{ y: -60 }"
        :exit="{ y: '100%' }">
        <CreateSpotForm ref="formRef" @submit="onSubmit" :loading="loading" />
    </AnimatedSheet>
</template>