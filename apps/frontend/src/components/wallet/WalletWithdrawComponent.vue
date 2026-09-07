<script setup lang="ts">
/**
 * Taking money out.
 *
 * **This screen is self-contained.** The only thing handed to it is `maxWithdraw`; it
 * asks the server for everything else, so a deep link, a reload, and a return from
 * Stripe's own iframe all render the same thing as arriving from the wallet. Nothing is
 * passed down through a store or a parent's fetch.
 *
 * `maxWithdraw` is a **cap for the first paint, not the truth.** It rides in the URL,
 * so it is user-editable and it is a snapshot: a booking that counted when the wallet
 * was drawn can be refunded a second later. `PaymentService::request_payout` recomputes
 * the balance under an advisory lock and refuses anything larger — which is why the
 * confirmation toast prints the figure the *server* returned.
 *
 * Withdrawing is not instant and does not pretend to be. The request writes a payout
 * row and returns; a worker makes the Stripe Transfer a moment later. The wallet shows
 * the row with its PENDING chip in the meantime, so there is nothing to wait for here.
 */
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { toast } from 'vue-sonner'
import { Landmark } from '@lucide/vue'
import FullScreenLayoutComponent from '../FullScreenLayoutComponent.vue'
import Button from '../ui/button/Button.vue'
import Spinner from '../ui/spinner/Spinner.vue'
import Separator from '../ui/separator/Separator.vue'
import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { IconBox } from '@/components/base/icon-box'
import { centsToEuros, eurosToCents, formatCents } from '@/lib/money.ts'
import { mountConnect } from '@/lib/connect'
import { viewKeys } from '@/api/viewApi'
import * as paymentApi from '@/api/paymentApi'

const { maxWithdraw } = defineProps<{
    maxWithdraw: number
}>()

/**
 * The smallest withdrawal, in cents.
 *
 * Mirrors `shared::domain_models::payment::payout::MIN_CENTS`, which is enforced twice
 * server-side — once by the request validator and once by `policy::payout` under the
 * lock. This copy is only so the button can explain itself before a round trip.
 */
const MIN_CENTS = 1000

const router = useRouter()
const queryClient = useQueryClient()

// Whether this host can be paid at all. Its own query rather than a prop: this is the
// question the screen exists to answer for itself, and it is answered from Stripe live
// — a host who finished onboarding thirty seconds ago must see the form.
const {
    data: connectStatus,
    isPending: statusPending,
    isError: statusFailed,
    refetch: refetchStatus,
} = useQuery({
    queryKey: ['connect', 'account'],
    queryFn: paymentApi.fetchConnectStatus,
    // Onboarding happens in an iframe on this page, so a cached "not yet" outlives the
    // moment it stops being true.
    staleTime: 0,
})

const amount = ref(String(centsToEuros(maxWithdraw).toFixed(2)))
const busy = ref(false)

/** What the field means, in cents. `0` for anything unparseable, which fails the same. */
const cents = computed(() => {
    const euros = Number(amount.value.replace(',', '.'))
    return Number.isFinite(euros) ? eurosToCents(euros) : 0
})

/** Nothing at all can be withdrawn — not a validation failure, a different screen. */
const belowMinimumBalance = computed(() => maxWithdraw < MIN_CENTS)

/**
 * Why the amount is refusable, or `null`. Rendered under the field and mirrored by the
 * disabled button, so the button is never a dead end without an explanation.
 */
const problem = computed(() => {
    if (cents.value < MIN_CENTS) return `The smallest withdrawal is ${formatCents(MIN_CENTS)}.`
    if (cents.value > maxWithdraw) return `You have ${formatCents(maxWithdraw)} available.`
    return null
})

// The chips. Anything over the balance would be a button that cannot be pressed, so
// only the ones that fit are offered — and `All` is last because it is the common case.
const quickPicks = computed(() => [25_00, 50_00, 100_00].filter((c) => c <= maxWithdraw))

function pick(picked: number) {
    amount.value = centsToEuros(picked).toFixed(2)
}

/** Selecting the whole field on focus: the field arrives pre-filled, and retyping is the point. */
function selectAll(event: FocusEvent) {
    (event.target as HTMLInputElement).select()
}

async function withdraw() {
    if (problem.value || busy.value) return
    busy.value = true

    try {
        // The server's figure, not `cents.value` — it recomputed under the lock and may
        // have paid less.
        const paid = await paymentApi.requestPayout(cents.value)

        // Both are stale the moment the payout commits: the balance is lower and the
        // history has a row it did not have. `requestPayout` recorded the version, so
        // the refetch waits for the projection rather than racing it.
        await Promise.all([
            queryClient.invalidateQueries({ queryKey: viewKeys.balance }),
            queryClient.invalidateQueries({ queryKey: viewKeys.wallet }),
        ])

        // `replace`, never `push`: the back button must not return to a withdraw form
        // for money that is already gone.
        router.replace({ name: 'wallet' })
        toast.success(`${formatCents(paid)} on its way`, {
            description: 'It should reach your bank in a couple of days.',
        })
    } catch (e: any) {
        // Includes the 422s — below the minimum, more than available, nothing settled.
        // All of them are terminal and none is worth a retry, so the message is the
        // whole response.
        toast.error("Couldn't withdraw", { description: e.message })
    } finally {
        busy.value = false
    }
}

// ─── Stripe Connect's embedded components ───────────────────────────────────
//
// Two of them, and they are the two halves of one job: `account-onboarding` collects
// identity and a bank account the first time, `account-management` lets a host change
// that bank account afterwards. Both render in Stripe's own iframes — no bank detail
// ever touches this app.

const onboardingEl = ref<HTMLDivElement | null>(null)
const managementEl = ref<HTMLDivElement | null>(null)
const changingBank = ref(false)

// The container appears with the branch it belongs to, so mounting is driven by the ref
// rather than by `onMounted` — which would run before `v-if` has produced anything.
//
// Each mount gets its own Connect instance and its own Account Session; the reason that
// matters is written out over `mountConnect`.
watch(onboardingEl, (el) => {
    if (!el) return
    // Fired when the host finishes or backs out. Either way the answer to "can this
    // person be paid" may have changed, and only Stripe knows.
    mountConnect('account-onboarding', el).setOnExit(() => void refetchStatus())
})

watch(managementEl, (el) => {
    if (el) mountConnect('account-management', el)
})
</script>

<template>
    <FullScreenLayoutComponent title="Withdraw" :description="`${formatCents(maxWithdraw)} available`"
        @close="router.replace({ name: 'wallet' })">
        <template #main>
            <!-- Asking Stripe, which is a round trip on every visit — see
                 `ConnectService::status` for why it is not a cached column. -->
            <Surface v-if="statusPending" variant="none" size="none" class="items-center gap-3 py-16 text-center">
                <Spinner class="size-6" />
                <Text size="sm">Loading…</Text>
            </Surface>

            <Surface v-else-if="statusFailed" variant="none" size="none" class="items-center gap-3 py-16 text-center">
                <Title size="sm" weight="semibold">We couldn't check your payout details.</Title>
                <Button variant="outline" @click="refetchStatus()">Try again</Button>
            </Surface>

            <!-- No country on the profile, so there is nothing to create an account
                 with. Its own screen rather than a failed button press: Stripe fixes
                 the country permanently at creation, so it has to be right first. -->
            <div v-else-if="connectStatus?.state === 'needs_country'" class="py-8 text-center">
                <Title size="sm" weight="extrabold">One thing first</Title>
                <Text class="mx-auto mt-1 max-w-xs leading-[1.45]">
                    Stripe needs to know which country you bank in before it can open your
                    payout account. It can't be changed afterwards, so we ask you rather
                    than guess.
                </Text>
                <Button class="mt-4 font-bold" @click="router.push({ name: 'profile-edit' })">
                    Add it to your profile
                </Button>
            </div>

            <!-- Not ready to be paid. One branch for both `none` and `onboarding`: the
                 difference between "never started" and "started and stopped" is
                 Stripe's to explain inside the component, which resumes where the host
                 left off. -->
            <template v-else-if="connectStatus?.state !== 'enabled'">
                <Surface orientation="horizontal" class="items-start gap-3">
                    <IconBox tone="brand">
                        <Landmark />
                    </IconBox>
                    <div>
                        <Title size="sm" weight="extrabold">Set up payouts</Title>
                        <Text class="leading-[1.4]">
                            Stripe needs a few details and a bank account before it can pay you.
                            Everything below is theirs — we never see your bank details.
                        </Text>
                    </div>
                </Surface>

                <!-- The card is ours, the contents are Stripe's. Their component fills
                     whatever box it is given and carries no padding of its own, so the
                     inset has to come from here — and it has to be *inside* a box
                     painted `bg-card`, or it would only shrink the panel and show page
                     background around it. Same `--card` on both sides, so the seam
                     doesn't show. -->
                <div ref="onboardingEl" class="rounded-md bg-card p-4"></div>
            </template>

            <!-- Onboarded, but there is nothing to take out yet. Distinct from a
                 validation failure: no amount would work, so no form is offered. -->
            <div v-else-if="belowMinimumBalance" class="py-10 text-center">
                <Title size="sm" weight="semibold">Not enough to withdraw yet</Title>
                <Text class="mt-1">
                    Withdrawals start at {{ formatCents(MIN_CENTS) }}. You have {{ formatCents(maxWithdraw) }}.
                </Text>
            </div>

            <template v-else>
                <!-- The amount. One big field, because it is the only thing on this
                     screen the host is actually deciding. -->
                <div class="flex items-center justify-center gap-1 pt-4 pb-1">
                    <Title as="span" weight="extrabold" tone="muted" class="text-[34px] leading-none">€</Title>
                    <input v-model="amount" inputmode="decimal" @focus="selectAll"
                        class="w-[6ch] bg-transparent text-[44px] leading-none font-extrabold tracking-tight outline-none"
                        aria-label="Amount to withdraw" />
                </div>

                <Text weight="semibold" tone="destructive" class="min-h-5 text-center">
                    {{ problem }}
                </Text>

                <div class="flex justify-center gap-2">
                    <Button v-for="option in quickPicks" :key="option" variant="outline" size="sm"
                        class="min-w-16 font-bold" @click="pick(option)">
                        {{ formatCents(option) }}
                    </Button>
                    <Button variant="outline" size="sm" class="min-w-16 font-bold" @click="pick(maxWithdraw)">
                        All
                    </Button>
                </div>

                <Separator />

                <!-- The overview. Live, so the figure being confirmed is the figure in
                     the field — there is no second screen where they could diverge. -->
                <Surface as="dl" size="sm" class="gap-2 py-2.5">
                    <div class="flex items-center justify-between">
                        <Text as="dt" size="sm">Amount</Text>
                        <Title as="dd" size="sm">{{ formatCents(Math.max(cents, 0)) }}</Title>
                    </div>
                    <div class="flex items-center justify-between">
                        <Text as="dt" size="sm">Fee</Text>
                        <Title as="dd" size="sm" tone="success">Free</Title>
                    </div>
                    <!-- Omitted rather than faked when Stripe hands back no external
                         account, which happens while one is still being verified. -->
                    <div v-if="connectStatus.bankLast4" class="flex items-center justify-between">
                        <Text as="dt" size="sm">To</Text>
                        <Title as="dd" size="sm">•••• {{ connectStatus.bankLast4 }}</Title>
                    </div>
                    <div class="flex items-center justify-between">
                        <Text as="dt" size="sm">Arrives</Text>
                        <Title as="dd" size="sm">In a couple of days</Title>
                    </div>
                </Surface>

                <!-- Changing where the money goes. Mounted on demand: it is a whole
                     Stripe iframe, and almost nobody opens it. -->
                <div>
                    <Button v-if="!changingBank" variant="link" size="sm" class="px-0"
                        @click="changingBank = true">
                        Change bank account
                    </Button>
                    <div v-else ref="managementEl" class="rounded-md bg-card p-4"></div>
                </div>
            </template>
        </template>

        <template #footer>
            <Button v-if="connectStatus?.state === 'enabled' && !belowMinimumBalance"
                class="h-11 w-full font-bold" :disabled="!!problem || busy" @click="withdraw">
                <Spinner v-if="busy" class="size-4" />
                {{ busy ? 'Withdrawing…' : `Withdraw ${formatCents(Math.max(cents, 0))}` }}
            </Button>
        </template>
    </FullScreenLayoutComponent>
</template>
