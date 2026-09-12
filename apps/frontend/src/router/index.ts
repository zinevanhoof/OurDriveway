import HomeView from "@/views/HomeView.vue";
import { createWebHistory, createRouter } from "vue-router";
import type { RouteLocationNormalized } from "vue-router";
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
import EditProfileView from "@/views/EditProfileView.vue";
import ChangePasswordView from "@/views/ChangePasswordView.vue";
import VerifyEmailView from "@/views/VerifyEmailView.vue";
import CheckoutView from "@/views/CheckoutView.vue";
import WalletView from "@/views/wallet/WalletView.vue";
import WalletWithdrawView from "@/views/wallet/WalletWithdrawView.vue";

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
    path: "/wallet",
    name: "wallet",
    component: WalletView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/wallet/withdraw/:maxWithdraw",
    name: "wallet-withdraw",
    component: WalletWithdrawView,
    // Params are strings; the cap is money in cents.
    props: (route: RouteLocationNormalized) => ({
      maxWithdraw: Number(route.params.maxWithdraw),
    }),
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
    path: "/profile/edit",
    name: "profile-edit",
    component: EditProfileView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile/password",
    name: "profile-password",
    component: ChangePasswordView,
    meta: {
      header: null,
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    // Reached from a link in an email, so it must work for someone who cannot log
    // in yet — being unverified is precisely why they can't.
    path: "/verify",
    name: "verify-email",
    component: VerifyEmailView,
    meta: {
      header: null,
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    // Paying for a held booking, and where Stripe sends the renter back after a redirect
    // method (Bancontact, iDEAL). Entry, reload, return and retry are all this one URL —
    // `?session_id=` is the only thing it needs, which is what makes a redirect
    // survivable. Requires auth: the session belongs to a renter and the endpoint
    // answers 404 to anyone else.
    path: "/checkout",
    name: "checkout",
    component: CheckoutView,
    meta: {
      header: null,
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
