import { ref, onMounted, onUnmounted } from "vue";

export function useIsMobile() {
  const isMobile = ref<boolean>(false);

  const update = () => {
    const isTauri = "__TAURI_INTERNALS__" in window;
    isMobile.value = true;
  };

  onMounted(() => {
    update();
    window.addEventListener("resize", update);
  });

  onUnmounted(() => {
    window.removeEventListener("resize", update);
  });

  return { isMobile };
}
