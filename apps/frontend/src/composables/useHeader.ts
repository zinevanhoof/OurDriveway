import { computed } from "vue";
import { useRoute } from "vue-router";
import type { RouteMeta } from "@/router";

export function useHeader() {
  const route = useRoute();

  const meta = computed(() => route.meta as RouteMeta);

  const title = computed(() => meta.value.title);
  const actions = computed(() => meta.value.headerActions);

  return {
    title,
    actions,
  };
}
