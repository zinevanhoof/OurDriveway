import { ref, watch, type Ref } from "vue";
import { useIntersectionObserver } from "@vueuse/core";

/**
 * Infinite scroll off one sentinel element below a `useInfiniteQuery` list.
 *
 * The observer records whether the sentinel is on screen; the watcher decides whether to
 * ask for another page. Split that way because an IntersectionObserver reports
 * *transitions*, and a first page that does not fill the screen leaves the sentinel
 * visible with no further callback ever coming. As watched state, the condition is
 * re-checked when `hasNextPage` flips, which keeps loading until the sentinel is pushed
 * off screen. `WalletComponent` spells the same thing out inline.
 */
export function useLoadMore(
  sentinel: Readonly<Ref<HTMLElement | null>>,
  query: {
    hasNextPage: Readonly<Ref<boolean>>;
    isFetchingNextPage: Readonly<Ref<boolean>>;
    fetchNextPage: () => unknown;
  },
) {
  const visible = ref(false);
  useIntersectionObserver(sentinel, ([entry]) => {
    visible.value = !!entry?.isIntersecting;
  });

  watch(
    [visible, query.hasNextPage, query.isFetchingNextPage],
    () => {
      if (visible.value && query.hasNextPage.value && !query.isFetchingNextPage.value) {
        void query.fetchNextPage();
      }
    },
    { immediate: true },
  );
}
