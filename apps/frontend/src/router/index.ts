import HomeView from "@/views/HomeView.vue";
import { createWebHistory, createRouter } from "vue-router";
import type { Component } from "vue";
import HeaderNotification from "@/components/header/HeaderNotification.vue";
import ProfileView from "@/views/ProfileView.vue";
import SearchView from "@/views/SearchView.vue";
import HeaderAddSpot from "@/components/header/HeaderAddSpot.vue";
import LoginView from "@/views/LoginView.vue";
import { useAuthStore } from "@/stores/auth";
import HeaderLogout from "@/components/header/HeaderLogout.vue";
import SpotsView from "@/views/SpotsView.vue";
import DetailedSpotViewOwned from "@/views/DetailedSpotViewOwned.vue";

type HeaderAction = {
  component: Component;
  props?: Record<string, any>;
};

export type RouteMeta = {
  title: string;
  headerActions: HeaderAction[];
  requiresAuth: boolean;
};

const routes = [
  {
    path: "/",
    name: "home",
    component: HomeView,
    meta: {
      title: "Home",
      headerActions: [
        {
          component: HeaderNotification,
        },
      ],
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/login",
    name: "login",
    component: LoginView,
    meta: {
      title: "Login",
      headerActions: [],
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    path: "/spots",
    name: "spots",
    component: SpotsView,
    meta: {
      title: "Parking spots",
      headerActions: [
        {
          component: HeaderAddSpot,
        },
        {
          component: HeaderNotification,
        },
      ],
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/:id",
    name: "spot",
    component: DetailedSpotViewOwned,
    props: true,
    meta: {
      title: "Parking spot",
      headerActions: [
        {
          component: HeaderNotification,
        },
      ],
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile",
    name: "profile",
    component: ProfileView,
    meta: {
      title: "Profile",
      headerActions: [
        {
          component: HeaderNotification,
        },
        {
          component: HeaderLogout,
        },
      ],
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/search",
    name: "search",
    component: SearchView,
    meta: {
      title: "Search parking spots",
      headerActions: [
        {
          component: HeaderNotification,
        },
      ],
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
