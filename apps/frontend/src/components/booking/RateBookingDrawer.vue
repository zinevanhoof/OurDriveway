<script setup lang="ts">
// One to five stars for a booking that is over. Rates the spot and, through it, the
// host — both averages are read off the same booking rows.
import { ref, watch } from "vue";
import { Star } from "@lucide/vue";
import { toast } from "vue-sonner";

import { useRateBooking } from "@/api/bookingApi";
import { Drawer, DrawerContent } from "@/components/ui/drawer";
import { Text, Title } from "@/components/base/text";
import Button from "@/components/ui/button/Button.vue";

const props = defineProps<{ bookingId: string | null; spotTitle: string | null }>();
const open = defineModel<boolean>("open", { default: false });

const rating = ref(0);
watch(open, (o) => o && (rating.value = 0));

const { mutate, isPending } = useRateBooking();

function submit() {
  if (!props.bookingId || !rating.value) return;
  mutate(
    { bookingId: props.bookingId, rating: rating.value },
    {
      onSuccess: () => {
        toast.success("Thanks for rating");
        open.value = false;
      },
      onError: (e: any) => toast.error("Couldn't save your rating", { description: e.message }),
    },
  );
}
</script>

<template>
  <Drawer v-model:open="open">
    <DrawerContent class="data-[vaul-drawer-direction=bottom]:mb-15">
      <div class="m-4 space-y-4 text-center">
        <Title size="lg">How was your parking?</Title>
        <Text>{{ spotTitle ?? "Your booking" }}</Text>
        <div class="flex justify-center gap-2" role="radiogroup" aria-label="Rating">
          <button v-for="n in 5" :key="n" type="button" role="radio" :aria-checked="rating === n"
            :aria-label="`${n} star${n > 1 ? 's' : ''}`" @click="rating = n">
            <Star :size="36" :class="n <= rating ? 'fill-star text-star' : 'text-muted-foreground'" />
          </button>
        </div>
        <Button class="w-full" :disabled="!rating || isPending" @click="submit">Submit</Button>
      </div>
    </DrawerContent>
  </Drawer>
</template>
