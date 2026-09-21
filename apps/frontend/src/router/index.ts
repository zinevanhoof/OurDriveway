import { createWebHistory, createRouter } from "vue-router";
import type { RouteLocationNormalized } from "vue-router";
import { useAuthStore } from "@/stores/auth";

import HomeView from "@/views/HomeView.vue";
import SearchView from "@/views/SearchView.vue";
import NotificationsView from "@/views/NotificationsView.vue";

import LoginView from "@/views/auth/LoginView.vue";
import SignupView from "@/views/auth/SignupView.vue";
import VerifyEmailView from "@/views/auth/VerifyEmailView.vue";
import ForgotPasswordView from "@/views/auth/ForgotPasswordView.vue";
import ResetPasswordView from "@/views/auth/ResetPasswordView.vue";

import MySpotsView from "@/views/spot/MySpotsView.vue";
import AddSpotView from "@/views/spot/AddSpotView.vue";
import EditSpotView from "@/views/spot/EditSpotView.vue";
import ManageSpotView from "@/views/spot/ManageSpotView.vue";
import SpotBookingsView from "@/views/spot/SpotBookingsView.vue";

import MyBookingsView from "@/views/booking/MyBookingsView.vue";
import BookSpotView from "@/views/booking/BookSpotView.vue";
import CheckoutView from "@/views/booking/CheckoutView.vue";

import WalletView from "@/views/wallet/WalletView.vue";
import WalletWithdrawView from "@/views/wallet/WalletWithdrawView.vue";

import ProfileView from "@/views/profile/ProfileView.vue";
import EditProfileView from "@/views/profile/EditProfileView.vue";
import ChangePasswordView from "@/views/profile/ChangePasswordView.vue";

export type RouteMeta = {
  requiresAuth: boolean;
};

const routes = [
  {
    path: "/",
    name: "home",
    component: HomeView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/login",
    name: "login",
    component: LoginView,
    meta: {
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    path: "/signup",
    name: "signup",
    component: SignupView,
    meta: {
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    // Host side only. `/bookings` is the renter's mirror of this — one screen each,
    // because one screen with a tab per persona is two screens wearing a trenchcoat.
    path: "/spots",
    name: "spots",
    component: MySpotsView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    // Renter side: the bookings this user made, not the ones made on their spots.
    // Those are `/spot/:id/bookings`.
    path: "/bookings",
    name: "bookings",
    component: MyBookingsView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/add",
    name: "spot-add",
    component: AddSpotView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/:id",
    name: "spot",
    component: ManageSpotView,
    props: true,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    // Every booking on one spot, paged, under an upcoming/past tab. The manage screen
    // shows two of them and links here.
    path: "/spot/:id/bookings",
    name: "spot-bookings",
    component: SpotBookingsView,
    props: true,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    // What the bell opens. Opening it marks everything seen.
    path: "/notifications",
    name: "notifications",
    component: NotificationsView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/spot/edit/:id",
    name: "spot-edit",
    component: EditSpotView,
    props: true,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/wallet",
    name: "wallet",
    component: WalletView,
    meta: {
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
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile",
    name: "profile",
    component: ProfileView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile/edit",
    name: "profile-edit",
    component: EditProfileView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/profile/password",
    name: "profile-password",
    component: ChangePasswordView,
    meta: {
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    // Reached from a link in an email, so it must work for someone who cannot log
    // in yet — being unverified is precisely why they can't.
    path: "/verify-email",
    name: "verify-email",
    component: VerifyEmailView,
    meta: {
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    // The same bargain as `/verify`, only more so: not being able to log in is the
    // entire premise of both of these.
    path: "/forgot-password",
    name: "forgot-password",
    component: ForgotPasswordView,
    meta: {
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    // This path is also built into the mailed link by notification-service
    // (`reset_url`). The two spellings have to match exactly or every reset email
    // in the wild lands on a 404.
    path: "/reset-password",
    name: "reset-password",
    component: ResetPasswordView,
    meta: {
      requiresAuth: false,
    } satisfies RouteMeta,
  },
  {
    // Picking dates and slots on someone else's spot, then holding them. A route rather
    // than a sheet over the map because it is a screen's worth of work with its own
    // back-stack entry — and because it hands off to `/checkout`, so the two halves of
    // booking are now both real URLs.
    //
    // Not under `/spot/:id/`: those are the host's own screens for a spot they own, and
    // this is the renter's side.
    path: "/book/:id",
    name: "book",
    component: BookSpotView,
    props: true,
    meta: {
      requiresAuth: true,
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
      requiresAuth: true,
    } satisfies RouteMeta,
  },
  {
    path: "/search",
    name: "search",
    component: SearchView,
    meta: {
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

  // The mirror image: every public route is a way *into* a session — login,
  // signup, and the three emailed-link screens — so with one already open there
  // is nothing to do on them. Safe on a cold load from an email link, because
  // `bootstrap` awaits the session refresh before installing the router.
  if (!to.meta.requiresAuth && auth.isAuthenticated) {
    return { name: "home" };
  }
});
