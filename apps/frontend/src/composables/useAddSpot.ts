import { ref } from "vue";

const addSpotClicked = ref(false);

export function useAddSpot() {
  function trigger() {
    addSpotClicked.value = true;
  }

  function reset() {
    addSpotClicked.value = false;
  }

  return {
    addSpotClicked,
    trigger,
    reset,
  };
}
