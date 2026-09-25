<script setup lang="ts">
// A spot's rating, star and average — or nothing at all when nobody has rated it.
//
// Owns its query so any place that knows a spot id can drop it in: the detail sheet and
// the home screen's nearby cards. The key is per spot, so the same spot shown twice is
// one request.
import { computed } from "vue";
import { useQuery } from "@tanstack/vue-query";
import { Star } from "@lucide/vue";

import { fetchSpotSummary } from "@/api/viewApi";
import { viewKeys } from "@/api/keys";
import { isPlaceholder } from "@/lib/placeholders";
import { Text } from "@/components/base/text";

const props = defineProps<{ spotId: string }>();

const { data } = useQuery({
  queryKey: computed(() => viewKeys.spotSummary(props.spotId)),
  queryFn: () => fetchSpotSummary(props.spotId),
  // Skeleton cards render this with a placeholder id; there is nothing to ask about.
  enabled: computed(() => !isPlaceholder(props.spotId)),
});
</script>

<template>
  <Text v-if="data?.rating != null" class="flex items-center gap-1">
    <Star :size="16" class="fill-star text-star" />
    {{ data.rating.toFixed(1) }}
    <span class="text-muted-foreground">({{ data.ratings }})</span>
  </Text>
</template>
