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
    <!-- Sized to the screen rather than to its content, down every level, so only the
         lists inside the tabs scroll. `min-h-0` on each flex child is what lets it
         shrink below its content; without it the chain grows and the page scrolls.

         No bottom padding here: the lists scroll, so their `pb-3` is inside their own
         scroll boxes, and those run flush to the navbar. -->
    <div class="flex min-h-0 flex-1 flex-col px-4 pt-4">
        <Tabs default-value="my-parking-spots" class="min-h-0 flex-1 gap-5">
            <TabsList class="w-full group-data-horizontal/tabs:h-10">
                <TabsTrigger value="my-parking-spots" class="font-bold">
                    My parking spots
                </TabsTrigger>
                <TabsTrigger value="my-bookings" class="font-bold">
                    My bookings
                </TabsTrigger>
            </TabsList>
            <TabsContent value="my-parking-spots" class="flex min-h-0 flex-col">
                <MySpotsComponent />
            </TabsContent>
            <!-- `-mt-2`: this tab opens on a second row of tabs, and two tab bars 20px
                 apart read as unrelated. -->
            <TabsContent value="my-bookings" class="-mt-3 flex min-h-0 flex-col">
                <BookedSpotsComponent />
            </TabsContent>
        </Tabs>
    </div>
</template>