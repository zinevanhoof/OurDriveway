<script setup lang="ts">
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'

import MySpotsComponent from "@/components/MySpotsComponent.vue";
import BookedSpotsComponent from '@/components/BookedSpotsComponent.vue';
</script>

<template>
    <div class="space-y-2 mt-4 mx-4 pb-2">
        <Tabs default-value="my-parking-spots" class="gap-5">
            <TabsList class="w-full group-data-horizontal/tabs:h-10">
                <TabsTrigger value="my-parking-spots" class="font-bold">
                    My parking spots
                </TabsTrigger>
                <TabsTrigger value="bookings" class="font-bold">
                    Bookings
                </TabsTrigger>
            </TabsList>
            <TabsContent value="my-parking-spots">
                <MySpotsComponent />
            </TabsContent>
            <TabsContent value="bookings">
                <BookedSpotsComponent />
            </TabsContent>
        </Tabs>
    </div>
</template>