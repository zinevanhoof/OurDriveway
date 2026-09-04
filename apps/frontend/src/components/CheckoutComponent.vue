<script setup lang="ts">
/**
 * Paying for a held booking.
 *
 * The whole screen is driven by one thing: the `session_id` in the URL. Entry, reload,
 * the return from Bancontact and a retry after a decline all take the identical path —
 * ask the server what became of the session, then render one of three outcomes. There is
 * no mode flag and no branch on which query parameters are present, which is what makes a
 * redirect survivable: this component holds no state worth losing.
 *
 * It does not create the session. `BookingFormComponent` does, because it is the one that
 * knows which booking, and it navigates here with the id. That is also why the booking id
 * never appears in a URL — the server hands it back from the session when it is needed.
 *
 * What the renter is buying is read from Stripe (`getSession()`), not refetched from
 * view-service. The line item is written server-side at session creation and says
 * "Parking · 14 Aug, 09:00–11:00".
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useQuery } from "@tanstack/vue-query";
import { toast } from "vue-sonner";
import { CircleCheck, CircleX, ShieldCheck, Timer } from "@lucide/vue";
import Button from "@/components/ui/button/Button.vue";
import Separator from "@/components/ui/separator/Separator.vue";
import Spinner from "@/components/ui/spinner/Spinner.vue";
import { stripe } from "@/lib/stripe";
import { cssColorToHex, token } from "@/lib/theme";
import { fetchBooking, fetchMe, viewKeys } from "@/api/viewApi";
import * as paymentApi from "@/api/paymentApi";
import * as bookingApi from "@/api/bookingApi";
import type {
  StripeCheckoutLineItem,
  StripeCheckoutLoadActionsSuccess,
} from "@stripe/stripe-js";

const route = useRoute();
const router = useRouter();
// No `useAuthStore()` here any more: the only thing it supplied was the caller's id for
// the `ME` document's `user(id:)` lookup, and `/me` takes that from the token.

/** Everything this screen knows on arrival. */
const sessionId = computed(() => String(route.query.session_id ?? ""));

type Screen = "loading" | "paying" | "confirming" | "booked" | "expired" | "error";
const screen = ref<Screen>("loading");

const bookingId = ref("");
const message = ref("");
const busy = ref(false);
const canConfirm = ref(false);

// Straight off the Checkout Session — no view-service read.
const total = ref("");
const lineItems = ref<StripeCheckoutLineItem[]>([]);

const paymentEl = ref<HTMLDivElement | null>(null);
const contactEl = ref<HTMLDivElement | null>(null);
// Held outside the reactive state on purpose: it is a live SDK handle, not data to
// render, and wrapping it in a ref would only invite Vue to proxy something Stripe owns.
let actions: StripeCheckoutLoadActionsSuccess | null = null;

// A redirect payment leaves this page entirely — the webview navigates to the bank and the
// SPA is destroyed. Nothing is held across it, and nothing needs to be: Stripe returns to
// `checkout/return.html`, which bounces through the app's own scheme back to this route,
// and `load()` rebuilds everything from the session id. See `returnUrl` in api/paymentApi.ts
// and `catch_deep_link` in src-tauri/src/lib.rs.

// Prefill the email Stripe requires, so a logged-in renter doesn't retype an address we
// already hold. `/me` is the only endpoint that returns it, and it picks the row from
// the verified claim — where this used to ask for a user by id and rely on a
// field-level permission to blank the address for anyone else.
const { data: me } = useQuery({
  queryKey: viewKeys.me,
  queryFn: fetchMe,
});

onMounted(load);

async function load() {
  if (!sessionId.value) {
    screen.value = "error";
    message.value = "That checkout link is missing its session.";
    return;
  }

  try {
    const state = await paymentApi.sessionState(sessionId.value);
    bookingId.value = state.bookingId;

    if (state.status === "expired") {
      screen.value = "expired";
      return;
    }
    if (state.status === "complete") {
      // Paid. The booking still confirms on the webhook, not on this — so wait for it.
      screen.value = "confirming";
      void awaitConfirmation();
      return;
    }
    if (!state.clientSecret) {
      screen.value = "error";
      message.value = "This checkout can't be resumed.";
      return;
    }

    await mount(state.clientSecret);
  } catch (e: any) {
    screen.value = "error";
    message.value = e.message;
  }
}

async function mount(clientSecret: string) {
  const sdk = (await stripe()).initCheckoutElementsSdk({
    clientSecret,
    // `profile` is still optional — null between signup and its projection — but the
    // email on it is not.
    defaultValues: { email: me.value?.profile?.email },
  });

  // Themed off the app's own tokens so the Element doesn't read as a third-party panel.
  // Converted because Tailwind 4 emits `oklch(...)` and Stripe accepts only HEX, rgb()
  // or hsl() — see `cssColorToHex`, shared with the Connect components in
  // `lib/connect.ts`, which need the identical treatment.
  sdk.changeAppearance({
    theme: "flat",
    variables: {
      colorPrimary: cssColorToHex(token("--primary")),
      colorBackground: cssColorToHex(token("--card")),
      colorText: cssColorToHex(token("--foreground")),
      colorDanger: cssColorToHex(token("--destructive")),
      borderRadius: token("--radius") || undefined,
    },
  });

  // Stripe owns whether the form is complete enough to submit; a local flag would only
  // ever be a worse copy of it.
  sdk.on("change", (session) => {
    canConfirm.value = session.canConfirm;
  });

  const loaded = await sdk.loadActions();
  if (loaded.type === "error") {
    screen.value = "error";
    message.value = loaded.error.message ?? "Couldn't load this checkout.";
    return;
  }

  actions = loaded.actions;
  const session = actions.getSession();
  total.value = session.total.total.amount;
  lineItems.value = session.lineItems;

  // A previous attempt on this same session — the renter is back here after a decline.
  if (session.lastPaymentError?.message) {
    message.value = session.lastPaymentError.message;
  }

  // Show the paying branch *before* mounting, then wait a tick for it to render. The two
  // containers only exist inside that branch, so mounting first hands Stripe a null and it
  // answers "Missing argument. Make sure to call mount() with a valid DOM element or
  // selector". Order matters here; the `await` is not optional.
  screen.value = "paying";
  await nextTick();

  if (!contactEl.value || !paymentEl.value) {
    screen.value = "error";
    message.value = "Couldn't render the payment form.";
    return;
  }

  sdk.createContactDetailsElement().mount(contactEl.value);
  sdk.createPaymentElement().mount(paymentEl.value);
}

async function pay() {
  if (!actions || busy.value) return;
  busy.value = true;
  message.value = "";

  try {
    // `if_required` is what keeps a card payment on this page: only a genuinely
    // redirect-based method (Bancontact, iDEAL) leaves, and it comes back to this same
    // URL. Nothing here decides the booking is paid — the webhook does.
    const result = await actions.confirm({ redirect: "if_required" });

    if (result.type === "error") {
      message.value = result.error.message ?? "Please try another payment method.";
      return;
    }

    // Reached only when no redirect was needed. Money has moved; wait for the webhook.
    screen.value = "confirming";
    void awaitConfirmation();
  } catch (e: any) {
    message.value = e.message;
  } finally {
    busy.value = false;
  }
}

/**
 * Waits for the booking to actually flip to `confirmed`.
 *
 * Stripe says the money arrived; only our webhook makes the booking real, so this is the
 * gap between the two. It gives up displaying after a while rather than claiming failure —
 * the payment succeeded either way, and a booking that confirms late still confirms.
 */
const POLL_MS = 1_500;
const GIVE_UP_MS = 30_000;
let poller: ReturnType<typeof setInterval> | undefined;

const { data: booking, refetch: refetchBooking } = useQuery({
  queryKey: computed(() => viewKeys.booking(bookingId.value ?? "")),
  queryFn: () => fetchBooking(bookingId.value!),
  enabled: computed(() => !!bookingId.value),
  staleTime: 0,
});

async function awaitConfirmation() {
  const started = Date.now();
  poller = setInterval(() => {
    if (booking.value?.status === "confirmed") {
      screen.value = "booked";
      stopPolling();
      return;
    }
    if (Date.now() - started > GIVE_UP_MS) {
      // Deliberately not an error. They paid; the booking will appear on its own.
      screen.value = "booked";
      message.value = "Your payment went through. The booking may take a moment to appear.";
      stopPolling();
      return;
    }
    void refetchBooking();
  }, POLL_MS);
}

function stopPolling() {
  clearInterval(poller);
}
onBeforeUnmount(stopPolling);

/** Hands the slots back now rather than making the next renter wait out the hold. */
async function giveUp() {
  if (busy.value) return;
  busy.value = true;
  try {
    await bookingApi.release(bookingId.value);
    toast.success("Released", { description: "Those times are back on the market." });
    void router.push("/search");
  } catch (e: any) {
    toast.error("Couldn't release that hold", { description: e.message });
  } finally {
    busy.value = false;
  }
}

// `cssColorToHex` and `token` moved to `lib/theme.ts` when Connect's embedded
// components needed the identical conversion — see the note in `lib/connect.ts`.
</script>

<template>
  <div class="mx-auto w-full max-w-lg px-4 py-6 space-y-3">
    <template v-if="screen === 'loading'">
      <div class="flex flex-col items-center gap-3 py-16 text-center">
        <Spinner class="size-6" />
        <div class="text-sm text-muted-foreground font-medium">Loading your checkout…</div>
      </div>
    </template>

    <template v-else-if="screen === 'confirming'">
      <div class="flex flex-col items-center gap-3 py-16 text-center">
        <div class="bg-accent text-accent-foreground p-3 rounded-lg"><Spinner class="size-6" /></div>
        <div class="text-lg font-extrabold">Confirming your payment…</div>
        <div class="text-sm text-muted-foreground font-medium max-w-sm">
          Your bank has told Stripe. We're waiting for Stripe to tell us — this normally
          takes a second or two.
        </div>
      </div>
    </template>

    <template v-else-if="screen === 'booked'">
      <div class="flex flex-col items-center gap-3 py-16 text-center">
        <div class="bg-accent text-accent-foreground p-3 rounded-lg"><CircleCheck class="size-6" /></div>
        <div class="text-lg font-extrabold">You're booked</div>
        <div class="text-sm text-muted-foreground font-medium max-w-sm">
          {{ message || "The spot is yours for the times you chose." }}
        </div>
        <Button class="h-11 font-bold" @click="router.push('/spots')">See my bookings</Button>
      </div>
    </template>

    <template v-else-if="screen === 'expired'">
      <div class="flex flex-col items-center gap-3 py-16 text-center">
        <div class="bg-accent text-accent-foreground p-3 rounded-lg"><Timer class="size-6" /></div>
        <div class="text-lg font-extrabold">This checkout expired</div>
        <div class="text-sm text-muted-foreground font-medium max-w-sm">
          Those times are back on the market. Nothing was charged.
        </div>
        <Button class="h-11 font-bold" @click="router.push('/search')">Find a spot</Button>
      </div>
    </template>

    <template v-else-if="screen === 'error'">
      <div class="flex flex-col items-center gap-3 py-16 text-center">
        <div class="bg-accent text-accent-foreground p-3 rounded-lg"><CircleX class="size-6" /></div>
        <div class="text-lg font-extrabold">Something went wrong</div>
        <div class="text-sm text-muted-foreground font-medium max-w-sm">{{ message }}</div>
        <Button class="h-11 font-bold" @click="router.push('/search')">Find a spot</Button>
      </div>
    </template>

    <!-- paying -->
    <template v-else>
      <div class="bg-card border border-border rounded-lg px-3.5 py-3.25 space-y-2">
        <div class="text-[15px] font-extrabold">Your booking</div>
        <!--
          Straight from the Checkout Session — the spot's name, its address, the times and
          a photo, with no request of ours behind any of it. payment-service asked
          spot-service for the title and photo once, when it created the session.

          `images` here are Stripe's own CloudFront URLs, not ours: it fetched our
          absolute URLs when the session was created and re-hosted the files. Rendered
          directly — there is no origin of ours to join on, and the photo is frozen at
          session creation, so editing the spot's images afterwards will not change it.
        -->
        <div v-for="item in lineItems" :key="item.id" class="space-y-2.5">
          <!--
            Same strip as SpotDetailDrawer: full-width pages so each photo snaps to
            centre, `no-scrollbar` because the snap points are the affordance.
          -->
          <div v-if="item.images?.length"
            class="flex h-40 gap-2 overflow-x-auto snap-x snap-mandatory no-scrollbar">
            <img v-for="key in item.images" :key="key" :src="key" alt=""
              class="snap-center shrink-0 h-full w-full object-cover rounded-md bg-accent" />
          </div>

          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div class="text-sm font-semibold">{{ item.name }}</div>
              <!-- Wraps. It carries the times *and* the address, which is longer than
                   one line on a phone and is the part worth reading. -->
              <div v-if="item.description" class="text-xs text-muted-foreground font-medium">
                {{ item.description }}
              </div>
            </div>
            <span class="text-sm font-semibold shrink-0">{{ item.total.amount }}</span>
          </div>
        </div>
        <Separator />
        <div class="flex items-center justify-between font-bold">
          <span class="text-muted-foreground">Total</span>
          <span>{{ total }}</span>
        </div>
      </div>

      <div class="bg-card border border-border rounded-lg px-3.5 py-3.25 space-y-3">
        <div ref="contactEl" />
        <div ref="paymentEl" />
      </div>

      <div v-if="message"
        class="text-sm font-semibold text-destructive bg-card border border-border rounded-lg px-3.5 py-3">
        {{ message }}
      </div>

      <div class="flex items-center gap-2 text-xs text-muted-foreground font-medium px-1">
        <ShieldCheck class="size-4 shrink-0" />
        Test mode — no real money moves. Card 4242&nbsp;4242&nbsp;4242&nbsp;4242 works.
      </div>

      <div class="space-y-2 pt-1">
        <Button class="w-full h-11 font-bold" :disabled="busy || !canConfirm" @click="pay">
          Pay {{ total }}
        </Button>
        <Button variant="ghost" class="w-full h-11 font-bold" :disabled="busy" @click="giveUp">
          Give up these times
        </Button>
      </div>
    </template>
  </div>
</template>
