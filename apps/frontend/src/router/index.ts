import HomeView from "@/views/HomeView.vue";
import { createWebHistory, createRouter } from "vue-router";
import type { Component } from "vue";
import ProfileView from "@/views/ProfileView.vue";
import SearchView from "@/views/SearchView.vue";
import LoginView from "@/views/LoginView.vue";
import { useAuthStore } from "@/stores/auth";
import SpotsView from "@/views/SpotsView.vue";
import MobileHomeHeader from "@/components/header/MobileHomeHeader.vue";
import MobileProfileHeader from "@/components/header/MobileProfileHeader.vue";
import AddSpotView from "@/views/AddSpotView.vue";
import ManageSpotView from "@/views/ManageSpotView.vue";
import EditSpotView from "@/views/EditSpotView.vue";

export type RouteMeta = {
  header: Component | null;
  requiresAuth: boolean;
};

const routes = [
  {
    path: "/",
    name: "home",
    component: HomeView,
    meta: {
      header: MobileHomeHeader,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/login",
    name: "login",
    component: LoginView,
    meta: {
      header: null,
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    path: "/spots",
    name: "spots",
    component: SpotsView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/add",
    name: "spot-add",
    component: AddSpotView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/:id",
    name: "spot",
    component: ManageSpotView,
    props: true,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/edit/:id",
    name: "spot-edit",
    component: EditSpotView,
    props: true,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile",
    name: "profile",
    component: ProfileView,
    meta: {
      header: MobileProfileHeader,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/search",
    name: "search",
    component: SearchView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
];

export const router = createRouter({
  history: createWebHistory(),
  routes,
});

router.beforeEach(async (to) => {
  const auth = useAuthStore();

  if (to.meta.requiresAuth && !auth.isAuthenticated) {
    return { name: "login" };
  }
});
