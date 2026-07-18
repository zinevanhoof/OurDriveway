import { User } from "@/types/User";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

export const useAuthStore = defineStore("auth", () => {
  const accessToken = ref<string | null>(null);
  const user = ref<User | null>(null);

  const isAuthenticated = computed(() => accessToken.value !== null);

  function setAccessToken(token: string | null) {
    accessToken.value = token;
  }

  function setUser(value: User | null) {
    user.value = value;
  }

  function login(token: string, userInfo: User) {
    accessToken.value = token;
    user.value = userInfo;
  }

  function logout() {
    accessToken.value = null;
    user.value = null;
  }

  return {
    accessToken,
    user,
    isAuthenticated,
    setAccessToken,
    setUser,
    login,
    logout,
  };
});
