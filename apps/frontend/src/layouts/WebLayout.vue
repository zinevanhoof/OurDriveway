<script setup lang="ts">
import { nextTick, ref, type Component } from "vue";
import { Button } from "@/components/ui/button";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  BoxesIcon,
  CarIcon,
  CheckIcon,
  CloudIcon,
  CreditCardIcon,
  DatabaseIcon,
  GlobeIcon,
  HouseIcon,
  KeyRoundIcon,
  LinkIcon,
  MailIcon,
  MapPinIcon,
  RotateCcwIcon,
  ServerIcon,
  WalletIcon,
} from "@lucide/vue";

/**
 * The landing page for the web build on a screen bigger than a phone. Mounted by main.ts
 * instead of App. Phones and the Tauri app never see it: they get the app itself.
 *
 * The phone on this page IS the app, in an iframe at iPhone 17 size (402 by 874 CSS px).
 * An iframe and not a narrow div, because the app is built against a phone's viewport:
 * dialogs, drawers and selects portal to <body>, some bars are `fixed`, and sheets are
 * sized in `vh`. In a div they would all measure the browser window instead. The frame
 * is square on purpose, since a rounded one could not clip the WebGL map.
 *
 * The text beside the phone switches between the project story and the tutorial. Only
 * that text swaps: the iframe stays mounted, so the app keeps its state.
 *
 * Needs `frame-ancestors 'self'` and `frame-src 'self'` in the CSP (both Caddyfiles).
 */

// No hash: the section links put one in the address bar, and it means nothing to the
// app's router.
const src = location.pathname + location.search;

const repo = "https://github.com/zinevanhoof/OurDriveway";

// The GitHub mark. lucide dropped its brand icons, so it is inlined.
const githubMark =
  "M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z";

const scroller = ref<HTMLElement>();
const tutorial = ref(false);

async function showTutorial(on: boolean) {
  tutorial.value = on;
  await nextTick();
  scroller.value?.scrollTo({ top: 0 });
}

const nav = [
  { href: "#why", label: "Why a phone" },
  { href: "#backend", label: "Microservices" },
  { href: "#stack", label: "Stack" },
];

type Point = { icon: Component; title: string; text: string };

const native: Point[] = [
  {
    icon: GlobeIcon,
    title: "Network calls go through Rust",
    text: "That way the login cookie keeps working, even though the app isn't running in a normal browser.",
  },
  {
    icon: MapPinIcon,
    title: "Location comes from the phone",
    text: "The app asks for GPS access with the phone's own permission prompt.",
  },
  {
    icon: LinkIcon,
    title: "Bank payments bring you back",
    text: "After paying with something like Bancontact in your bank app, a deep link drops you back into the booking.",
  },
];

const host = [
  "List your driveway with photos, a price per hour and the hours it's free each week.",
  "Pause, edit or delete a listing whenever you want.",
  "See who booked your spot and when.",
  "Get paid through Stripe and withdraw your earnings from the wallet.",
];

const driver = [
  "Find spots on a map, or search by address.",
  "Book one or more half hour slots.",
  "Pay in the app with Stripe.",
  "Cancel up to an hour before and get your money back.",
];

const services = [
  { name: "user", job: "Accounts, logins, email checks and sessions." },
  { name: "spot", job: "Listings, opening hours and address lookup." },
  { name: "booking", job: "Bookings, holds on slots and cancellations." },
  { name: "payment", job: "Payments, refunds and payouts through Stripe." },
  { name: "view", job: "The data every screen reads, kept up to date from the others." },
  { name: "media", job: "Upload links for photos, which go straight to storage." },
  { name: "notification", job: "Sends the emails." },
];

const ideas = [
  {
    title: "Each service owns its data",
    text: "Only the booking service writes bookings. It saves the booking and a message about it in the same transaction, so the two can never disagree.",
  },
  {
    title: "Messages, not calls",
    text: "Services don't call each other to get work done. When a payment goes through, the booking service hears about it and confirms the booking. If one service is down, the rest keep going.",
  },
  {
    title: "One place to read from",
    text: "Every screen gets its data from the view service, which keeps a copy of everything shaped the way the app needs it. If that copy is ever wrong, it gets rebuilt by replaying all the messages from the start.",
  },
  {
    title: "No double bookings",
    text: "Two people tapping Book on the same slot at the same moment can't both get it. The database decides, and there are tests that race real requests against each other to prove it.",
  },
];

const request: Point[] = [
  {
    icon: CloudIcon,
    title: "Cloudflare",
    text: "Every request goes through Cloudflare first. It handles HTTPS, caches the app's files and keeps the server's address hidden. The server only accepts connections that carry Cloudflare's client certificate, so nobody can go around it. Photos are stored in Cloudflare R2 and uploaded straight from your phone.",
  },
  {
    icon: ServerIcon,
    title: "k3s and Traefik",
    text: "The server is a single VPS running k3s, a small version of Kubernetes. Traefik sends each request to the right microservice and puts a rate limit on things like logins and signups. The whole setup is one Helm chart.",
  },
  {
    icon: BoxesIcon,
    title: "Seven Rust microservices",
    text: "Each one is written in Rust with axum and tokio. None of them keep state in memory, so handling more traffic just means running more copies of the one that's busy.",
  },
  {
    icon: DatabaseIcon,
    title: "PostgreSQL and NATS",
    text: "Each service has its own database. Here that's PostgreSQL, to keep the server small. The same code also runs on YugabyteDB, a database that spreads over many machines and speaks the same language. Messages between services go through NATS JetStream.",
  },
];

const stack = [
  { name: "App", items: ["Vue 3", "TypeScript", "Vite", "Tailwind CSS 4", "shadcn-vue", "TanStack Query", "MapLibre"] },
  { name: "Mobile", items: ["Tauri 2", "Android", "System webview"] },
  { name: "Backend", items: ["Rust", "Microservices", "axum", "tokio", "diesel", "garde", "argon2"] },
  { name: "Data", items: ["PostgreSQL", "YugabyteDB", "NATS JetStream"] },
  { name: "Hosting", items: ["Cloudflare", "k3s", "Helm", "Traefik", "Caddy", "GitHub Actions"] },
  { name: "Outside services", items: ["Stripe", "Stripe Connect", "Resend", "LocationIQ", "Cloudflare R2"] },
];

const steps: Point[] = [
  {
    icon: MailIcon,
    title: "Sign up with your real email",
    text: "Use an address you can open. You'll get a real email with a link to verify your account. Click it and you're in.",
  },
  {
    icon: KeyRoundIcon,
    title: "Try forgot password",
    text: "Log out and tap Forgot password on the login screen. A reset link lands in your inbox, just like in any other app.",
  },
  {
    icon: MapPinIcon,
    title: "Book a spot",
    text: "Open the map, pick a spot and choose one or more half hour slots. Your slots are held for 15 minutes while you pay.",
  },
  {
    icon: CreditCardIcon,
    title: "Pay with a test card",
    text: "Stripe is in test mode, so use one of the test cards below. Nothing is charged, and a real card won't work here at all.",
  },
  {
    icon: RotateCcwIcon,
    title: "Cancel for a refund",
    text: "You can cancel a booking up to an hour before it starts. The refund goes back through Stripe, and you can watch the booking change. This only works for bookings you made yourself, not for the ones Sam already had.",
  },
  {
    icon: HouseIcon,
    title: "List your own driveway",
    text: "Add photos, a price per hour and the hours it's free. To get paid, set up payouts with Stripe from the wallet. Stripe is in test mode there too: use 000 000 0000 as the phone number and 000000 as the code, and pick the test options Stripe shows you.",
  },
  {
    icon: WalletIcon,
    title: "Withdraw your earnings",
    text: "The money from a booking becomes available a day after the booking ends. Then you can withdraw it from the wallet.",
  },
];

const cards = [
  { number: "4242 4242 4242 4242", result: "Pays straight away." },
  { number: "4000 0027 6000 3184", result: "Asks for an extra check first. Tap Complete in the popup." },
  { number: "4000 0000 0000 0002", result: "Always gets declined, so you can see how that looks." },
];
</script>

<template>
  <div ref="scroller" class="flex-1 overflow-y-auto scroll-smooth bg-background text-foreground">
    <header class="sticky top-0 z-10 border-b bg-card/95 backdrop-blur">
      <div class="mx-auto flex h-14 max-w-6xl items-center gap-6 px-6">
        <button type="button" class="flex items-center gap-2 font-semibold tracking-tight" @click="showTutorial(false)">
          <img src="/ourdriveway-icon.svg" alt="" class="size-7" />
          OurDriveway
        </button>
        <nav class="ml-auto flex items-center gap-6">
          <template v-if="!tutorial">
            <a v-for="link in nav" :key="link.href" :href="link.href"
              class="hidden text-sm text-muted-foreground transition-colors hover:text-foreground lg:block">
              {{ link.label }}
            </a>
          </template>
          <div class="flex gap-2">
            <Button v-if="!tutorial" size="sm" class="px-3" @click="showTutorial(true)">How to try it</Button>
            <Button v-else variant="outline" size="sm" class="px-3" @click="showTutorial(false)">
              <ArrowLeftIcon />
              Back to the project
            </Button>
            <Button as="a" :href="repo" target="_blank" rel="noopener" variant="outline" size="sm" class="px-3">
              <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path :d="githubMark" /></svg>
              GitHub
            </Button>
          </div>
        </nav>
      </div>
    </header>

    <!-- Two columns from `lg`: the text on the left, the phone pinned on the right while
         it scrolls. Below that one column, with the phone between the intro and the rest.
         Same iframe element either way, so resizing never reloads the app. -->
    <div class="mx-auto grid max-w-6xl gap-x-16 px-6 lg:grid-cols-[minmax(0,1fr)_auto] xl:gap-x-24">
      <!-- The tutorial -->
      <section v-if="tutorial" class="animate-in fade-in pt-16 pb-24 duration-300 lg:pt-20">
        <p class="text-sm font-medium text-primary">How to try it</p>
        <h1 class="mt-3 text-4xl font-semibold tracking-tight text-balance">Everything here is real. Except the money.</h1>
        <p class="mt-6 text-lg leading-8 text-foreground/80">
          The emails, the database and the payments are all real. Stripe runs in test mode though, so
          you pay with test cards and nothing is ever charged. Follow these steps in the phone on the
          right and you'll see every part of the app.
        </p>

        <div class="mt-8 flex gap-3 rounded-xl border border-primary/20 bg-accent p-4 text-sm leading-6">
          <RotateCcwIcon class="mt-0.5 size-5 shrink-0 text-primary" />
          <p class="text-foreground/80">
            <span class="font-medium text-foreground">The demo resets every hour.</span>
            On the hour, everything is wiped and filled with the demo data again. That includes
            accounts you made yourself, so after a reset you'll need to sign up again. The app is
            also offline for a minute or two while that happens.
          </p>
        </div>

        <div class="mt-8 rounded-xl border bg-card p-5">
          <p class="font-medium">Just want a quick look?</p>
          <p class="mt-1 text-sm leading-6 text-muted-foreground">
            Log in with the demo account. Sam already has driveways, bookings and money in the
            wallet, so there's something on every screen.
          </p>
          <dl class="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            <dt class="text-muted-foreground">Email</dt>
            <dd class="font-mono font-medium">sam@example.com</dd>
            <dt class="text-muted-foreground">Password</dt>
            <dd class="font-mono font-medium">Demo1234!</dd>
          </dl>
          <p class="mt-4 text-sm leading-6 text-muted-foreground">
            One thing to know: Sam's existing bookings were paid with fake payments that only exist
            to fill the demo. Stripe has nothing to refund for those, so cancelling them won't work.
            Bookings you make yourself can be cancelled just fine.
          </p>
          <p class="mt-4 text-sm leading-6 text-muted-foreground">
            Want to see the emails too? Then sign up with your own address and follow the steps
            below.
          </p>
        </div>

        <div class="mt-4 rounded-xl border bg-card p-5">
          <p class="font-medium">Where's my profile?</p>
          <p class="mt-1 text-sm leading-6 text-muted-foreground">
            It isn't in the bar at the bottom. Go to Home and tap your picture in the top right
            corner. That's where you edit your name, photo and licence plates.
          </p>
        </div>

        <ol class="mt-10">
          <li v-for="(step, i) in steps" :key="step.title" class="flex gap-4">
            <div class="flex flex-col items-center">
              <span class="flex size-10 shrink-0 items-center justify-center rounded-full border bg-card text-primary shadow-xs">
                <component :is="step.icon" class="size-5" />
              </span>
              <span v-if="i < steps.length - 1" class="my-2 w-px flex-1 bg-border" />
            </div>
            <div class="pt-2" :class="{ 'pb-8': i < steps.length - 1 }">
              <h2 class="font-medium">
                <span class="text-muted-foreground">{{ i + 1 }}.</span> {{ step.title }}
              </h2>
              <p class="mt-1 text-sm leading-6 text-muted-foreground">{{ step.text }}</p>

              <div v-if="step.icon === CreditCardIcon" class="mt-4 overflow-hidden rounded-xl border bg-card">
                <div v-for="card in cards" :key="card.number"
                  class="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b px-4 py-3 last:border-b-0">
                  <span class="font-mono text-sm font-medium tracking-wide">{{ card.number }}</span>
                  <span class="text-sm text-muted-foreground">{{ card.result }}</span>
                </div>
                <p class="bg-muted/60 px-4 py-3 text-xs leading-5 text-muted-foreground">
                  Use any date in the future, any three digit CVC and any name.
                </p>
              </div>
            </div>
          </li>
        </ol>

        <div class="mt-10 rounded-xl border bg-card p-5 text-sm leading-6 text-muted-foreground">
          <p class="font-medium text-foreground">Something broke?</p>
          <p class="mt-1">
            It's a one-person project, so that can happen. You can let me know by opening an issue on
            <a :href="`${repo}/issues`" target="_blank" rel="noopener"
              class="font-medium text-primary underline-offset-4 hover:underline">GitHub</a>.
          </p>
        </div>

        <Button variant="outline" size="lg" class="mt-10 px-4" @click="showTutorial(false)">
          <ArrowLeftIcon />
          Back to the project
        </Button>
      </section>

      <!-- The project: intro -->
      <section v-else class="animate-in fade-in pt-16 pb-12 duration-300 lg:pt-24">
        <p class="text-sm font-medium text-primary">Portfolio project</p>
        <h1 class="mt-3 text-4xl font-semibold tracking-tight text-balance sm:text-5xl">
          Rent out your driveway. Or find a spot to park.
        </h1>
        <p class="mt-6 text-lg leading-8 text-foreground/80">
          OurDriveway is a marketplace for private parking. Hosts list their driveway and the
          hours it's free. Drivers find it on a map, book a time slot and pay in the app. Behind
          it is a microservices backend written in Rust.
        </p>
        <p class="mt-4 leading-7 text-muted-foreground">
          The phone on this page is the real app, talking to the real backend. Sign up with your
          own email and try it out.
        </p>
        <div class="mt-8 flex flex-wrap gap-3">
          <Button size="lg" class="px-4" @click="showTutorial(true)">
            How to try it
            <ArrowRightIcon />
          </Button>
          <Button as="a" :href="repo" target="_blank" rel="noopener" variant="outline" size="lg" class="px-4">
            <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path :d="githubMark" /></svg>
            See the code on GitHub
          </Button>
        </div>
      </section>

      <aside class="lg:col-start-2 lg:row-span-2 lg:row-start-1">
        <div class="relative isolate flex flex-col items-center gap-3 pb-12 lg:sticky lg:top-14 lg:h-[calc(100dvh-3.5rem)] lg:justify-center lg:pb-0">
          <div aria-hidden="true" class="absolute inset-x-0 top-1/4 -z-10 h-1/2 rounded-full bg-primary/20 blur-3xl" />
          <div class="bg-foreground p-1.5 shadow-2xl">
            <iframe :src="src" title="OurDriveway app" allow="geolocation; payment"
              class="block h-[min(874px,calc(100dvh-8rem))] w-[402px] bg-background" />
          </div>
          <p class="text-sm text-muted-foreground">This is the real app. Go ahead and sign up.</p>
        </div>
      </aside>

      <!-- The project: the rest -->
      <div v-if="!tutorial" class="pb-24">
        <section id="why" class="scroll-mt-16 border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">Why a phone on a website?</h2>
          <p class="mt-4 leading-7 text-foreground/80">
            OurDriveway is built as a mobile app for Android, with the Play Store as the goal. I first
            planned a separate web version too, but the phone is the main version, so I kept it that
            way.
          </p>
          <p class="mt-4 leading-7 text-foreground/80">
            This website is just a showcase. It runs the same app in a frame, so you can try it
            without installing anything, or without an Android phone at all.
          </p>
        </section>

        <section class="border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">Not React Native. Not Flutter.</h2>
          <p class="mt-4 leading-7 text-foreground/80">
            OurDriveway started as a school project in React Native, with Expo and Firebase. That
            got something on screen fast, but everything important lived inside Firebase. So I
            rebuilt the whole thing from scratch.
          </p>
          <p class="mt-4 leading-7 text-foreground/80">
            The new app isn't Flutter, React Native or a native Android or iOS app. It's a Vue web
            app wrapped with Tauri 2. On a phone it runs in the system's own webview, and a small
            Rust layer handles the few things a webview can't do well by itself.
          </p>
          <ul class="mt-6 space-y-4">
            <li v-for="point in native" :key="point.title" class="flex gap-4">
              <span class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-accent text-primary">
                <component :is="point.icon" class="size-4.5" />
              </span>
              <div>
                <h3 class="font-medium">{{ point.title }}</h3>
                <p class="mt-0.5 text-sm leading-6 text-muted-foreground">{{ point.text }}</p>
              </div>
            </li>
          </ul>
          <p class="mt-6 leading-7 text-foreground/80">
            The result is one codebase for Android and the web. The app also stays small, because
            it uses the webview that's already on the phone instead of bringing its own engine.
          </p>
        </section>

        <section class="border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">What you can do</h2>
          <div class="mt-6 grid gap-4 sm:grid-cols-2">
            <div class="rounded-xl border bg-card p-5">
              <div class="flex items-center gap-2 font-medium">
                <HouseIcon class="size-5 text-primary" />
                As a host
              </div>
              <ul class="mt-4 space-y-3">
                <li v-for="item in host" :key="item" class="flex gap-2.5 text-sm leading-6 text-muted-foreground">
                  <CheckIcon class="mt-1 size-4 shrink-0 text-success" />
                  {{ item }}
                </li>
              </ul>
            </div>
            <div class="rounded-xl border bg-card p-5">
              <div class="flex items-center gap-2 font-medium">
                <CarIcon class="size-5 text-primary" />
                As a driver
              </div>
              <ul class="mt-4 space-y-3">
                <li v-for="item in driver" :key="item" class="flex gap-2.5 text-sm leading-6 text-muted-foreground">
                  <CheckIcon class="mt-1 size-4 shrink-0 text-success" />
                  {{ item }}
                </li>
              </ul>
            </div>
          </div>
        </section>

        <section id="backend" class="scroll-mt-16 border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">A microservices backend</h2>
          <p class="mt-4 leading-7 text-foreground/80">
            The backend isn't one big server. It's seven microservices, each a small Rust program with
            one job. Every service has its own database, gets deployed on its own and can run as many
            copies as it needs.
          </p>
          <div class="mt-6 overflow-hidden rounded-xl border bg-card">
            <div v-for="service in services" :key="service.name"
              class="grid gap-1 border-b px-4 py-3 last:border-b-0 sm:grid-cols-[8.5rem_1fr] sm:gap-4">
              <span class="font-mono text-sm font-medium text-primary">{{ service.name }}</span>
              <span class="text-sm text-muted-foreground">{{ service.job }}</span>
            </div>
          </div>

          <h3 class="mt-10 text-lg font-semibold tracking-tight">How they work together</h3>
          <div class="mt-4 grid gap-4 sm:grid-cols-2">
            <div v-for="idea in ideas" :key="idea.title" class="rounded-xl border bg-card p-5">
              <h4 class="font-medium">{{ idea.title }}</h4>
              <p class="mt-2 text-sm leading-6 text-muted-foreground">{{ idea.text }}</p>
            </div>
          </div>
        </section>

        <section class="border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">From Cloudflare to Rust</h2>
          <p class="mt-4 leading-7 text-foreground/80">
            Here's the path a request takes when you tap Book, from the first hop all the way down
            to the database.
          </p>
          <ol class="mt-8">
            <li v-for="(step, i) in request" :key="step.title" class="flex gap-4">
              <div class="flex flex-col items-center">
                <span class="flex size-10 shrink-0 items-center justify-center rounded-full border bg-card text-primary shadow-xs">
                  <component :is="step.icon" class="size-5" />
                </span>
                <span v-if="i < request.length - 1" class="my-2 w-px flex-1 bg-border" />
              </div>
              <div class="pt-2" :class="{ 'pb-8': i < request.length - 1 }">
                <h3 class="font-medium">{{ step.title }}</h3>
                <p class="mt-1 text-sm leading-6 text-muted-foreground">{{ step.text }}</p>
              </div>
            </li>
          </ol>
        </section>

        <section id="stack" class="scroll-mt-16 border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">The stack</h2>
          <dl class="mt-6 space-y-5">
            <div v-for="group in stack" :key="group.name" class="grid gap-2 sm:grid-cols-[9rem_1fr] sm:gap-4">
              <dt class="pt-1 text-sm font-medium text-muted-foreground">{{ group.name }}</dt>
              <dd class="flex flex-wrap gap-2">
                <span v-for="item in group.items" :key="item" class="rounded-full border bg-card px-3 py-1 text-sm">
                  {{ item }}
                </span>
              </dd>
            </div>
          </dl>
        </section>

        <section class="border-t py-14">
          <h2 class="text-2xl font-semibold tracking-tight">Where it stands</h2>
          <p class="mt-4 leading-7 text-foreground/80">
            This is a finished portfolio project, not a business. Everything works from start to finish:
            sign up, list a spot, book it, pay, cancel, get a refund and withdraw your earnings.
          </p>
          <p class="mt-4 leading-7 text-foreground/80">
            Some things a real launch would need are still missing, like database backups and
            running on more than one server. The Android build also gets less testing than the web
            version. I keep the full list of gaps in the
            <a :href="`${repo}#status-and-known-gaps`" target="_blank" rel="noopener"
              class="font-medium text-primary underline-offset-4 hover:underline">README</a>.
          </p>
          <Button size="lg" class="mt-8 px-4" @click="showTutorial(true)">
            How to try it
            <ArrowRightIcon />
          </Button>
        </section>
      </div>
    </div>

    <footer class="border-t">
      <div class="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-4 px-6 py-8 text-sm text-muted-foreground">
        <p>Built by Zine Van Hoof as a portfolio project.</p>
        <a :href="repo" target="_blank" rel="noopener" class="flex items-center gap-2 transition-colors hover:text-foreground">
          <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" class="size-4"><path :d="githubMark" /></svg>
          zinevanhoof/OurDriveway
        </a>
      </div>
    </footer>
  </div>
</template>
