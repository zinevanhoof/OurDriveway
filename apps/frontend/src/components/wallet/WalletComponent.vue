<script setup lang="ts">
/**
 * Every movement of the signed-in user's money, a month at a time.
 *
 * **A month is a page.** The screen groups by month, so the history is fetched by month
 * too: each response carries its own totals and names the next older month that holds
 * anything, which is the cursor. That is why there is no client-side grouping here any
 * more and no offset to drift — and why scrolling to the bottom can stop, rather than
 * walking backwards forever through months with nothing in them.
 *
 * Every figure comes from view-service. payment-service writes money and view-service
 * reads it, so the balance in the header and the rows underneath it are one query
 * apiece against one projection, and cannot tell two different stories.
 *
 * Sentences are composed here rather than served. The API sends the spot's title and
 * the booking's slots; the labels are ours, and the times print in the spot's own wall
 * clock through the same helpers every other booking on screen uses.
 */
import { computed, ref, useTemplateRef, watchEffect } from 'vue'
import { useInfiniteQuery, useQuery } from '@tanstack/vue-query'
import { useIntersectionObserver } from '@vueuse/core'
import { ArrowUpRight, Banknote, CarFront, Clock, Landmark, RotateCcw } from '@lucide/vue'
import { formatCents } from '@/lib/money'
import { formatDay, formatSlots, sortedDays } from '@/lib/bookingDates'
import { fetchBalance, fetchWallet, viewKeys } from '@/api/viewApi'
import type { WalletTransaction } from '@/types/view'
import Button from '../ui/button/Button.vue'
import { useRouter } from 'vue-router'

const router = useRouter()

const { data: balance } = useQuery({
    queryKey: viewKeys.balance,
    queryFn: fetchBalance,
})

// `pageParam` is a month string, and `undefined` on the first page means "whichever
// month it is where the server is". The client deliberately does not compute that
// itself: the month a row falls in is decided by the same boundaries the query uses.
const { data, fetchNextPage, hasNextPage, isFetchingNextPage, isPending } = useInfiniteQuery({
    queryKey: viewKeys.wallet,
    queryFn: ({ pageParam }) => fetchWallet(pageParam),
    initialPageParam: undefined as string | undefined,
    // Null ends the list. A month with nothing in it is never requested, because the
    // server answers with the next month that actually holds something.
    getNextPageParam: (last) => last.nextMonth ?? undefined,
})

const pages = computed(() => data.value?.pages ?? [])
// What actually renders. The current month is served whether or not it holds anything,
// so page one is regularly empty; the tiles below still want it, the list does not.
const months = computed(() => pages.value.filter((p) => p.transactions.length))
const available = computed(() => balance.value?.availableCents ?? 0)
const pending = computed(() => balance.value?.pendingCents ?? 0)

// The two tiles are this month's, which is the first page by construction.
const monthIn = computed(() => pages.value[0]?.inCents ?? 0)
const monthOut = computed(() => pages.value[0]?.outCents ?? 0)
const currentMonthShort = computed(() =>
    new Date().toLocaleString(navigator.language, { month: 'short' }).toUpperCase()
)

const kindStyle = {
    in: { icon: Banknote, chip: 'bg-accent text-primary' },
    refund: { icon: RotateCcw, chip: 'bg-accent text-primary' },
    out: { icon: CarFront, chip: 'bg-muted' },
    payout: { icon: Landmark, chip: 'bg-muted' }
}

const signed = (cents: number) => `${cents >= 0 ? '+' : '−'}${formatCents(Math.abs(cents))}`

const dayFormat = new Intl.DateTimeFormat(navigator.language, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
})

/** `"2026-08"` -> `August 2026`, in the viewer's locale. */
function monthLabel(month: string): string {
    const [year, index] = month.split('-').map(Number)
    return new Date(year, index - 1).toLocaleString(navigator.language, {
        month: 'long',
        year: 'numeric',
    })
}

/** What the month came to, ignoring withdrawals — the same rule as the tiles. */
function net(page: { inCents: number; outCents: number }): string {
    return signed(page.inCents - page.outCents)
}

/** The spot, or the least-wrong stand-in for one. */
function title(tx: WalletTransaction): string {
    if (tx.kind === 'payout') return 'Withdrawal'
    // Null while the spot has not been projected here yet, and for a spot deleted long
    // enough ago to be gone. The row is still a real movement of money, so it shows.
    return tx.title ?? 'Parking'
}

/**
 * When it was for — the booked slots in the spot's own zone, not the viewer's.
 *
 * A payout has no booking, so it says when it was *made* instead.
 */
function sub(tx: WalletTransaction): string {
    if (tx.kind === 'payout') return dayFormat.format(new Date(tx.occurredAt))

    const [day] = sortedDays({ booked: tx.booked })
    const when = day ? `${formatDay(day[0], tx.timezone)} · ${formatSlots(day[1])}` : ''
    if (tx.kind !== 'refund') return when
    return when ? `Refunded · ${when}` : 'Refunded'
}

// Infinite scroll: one sentinel below the last month. The observer records whether it
// is on screen; the watcher decides whether to ask for another month.
//
// Split that way because an IntersectionObserver reports *transitions*, and the
// sentinel does not always make one. A quiet current month renders almost nothing, so
// the sentinel is already in view on mount — before the first page has landed and
// `hasNextPage` is still false — and it never leaves, so no second callback ever comes
// and the older months are unreachable. As a watched state instead, the condition is
// re-checked when `hasNextPage` flips, which also walks several short months in a row
// until the sentinel is finally pushed off screen.
const sentinel = useTemplateRef<HTMLElement>('sentinel')
const sentinelVisible = ref(false)
useIntersectionObserver(sentinel, ([entry]) => {
    sentinelVisible.value = !!entry?.isIntersecting
})

watchEffect(() => {
    if (sentinelVisible.value && hasNextPage.value && !isFetchingNextPage.value) {
        void fetchNextPage()
    }
})
</script>


<template>
    <div class="space-y-4 px-4 py-2">
        <h1 class="text-xl font-extrabold">Wallet</h1>

        <!-- Balance -->
        <div class="flex flex-col gap-4 rounded-md bg-primary p-4 text-primary-foreground">
            <div>
                <div class="text-[11.5px] font-semibold opacity-80">AVAILABLE TO WITHDRAW</div>
                <div class="text-[34px] leading-none font-extrabold">{{ formatCents(available) }}</div>
            </div>

            <div v-if="pending > 0" class="flex items-center gap-2 rounded-md bg-white/[0.14] px-3 py-2">
                <Clock class="size-4 shrink-0 opacity-90" />
                <span class="text-[12px] font-semibold leading-[1.35] opacity-95">
                    {{ formatCents(pending) }} pending — clears 24h after each booking ends
                </span>
            </div>

            <Button class="h-11 bg-primary-foreground text-primary font-bold"
                @click="router.push({ name: 'wallet-withdraw', params: { maxWithdraw: available } })">
                <ArrowUpRight class="size-4.5" />
                Withdraw
            </Button>
        </div>

        <!-- Month totals -->
        <div class="grid grid-cols-2 gap-2">
            <div class="rounded-md border border-border bg-card px-3 py-2">
                <div class="text-xs font-bold">MONEY IN · {{ currentMonthShort }}</div>
                <div class="text-xl font-extrabold text-success">{{ formatCents(monthIn) }}</div>
            </div>
            <div class="rounded-md border border-border bg-card px-3 py-2">
                <div class="text-xs font-bold">MONEY OUT · {{ currentMonthShort }}</div>
                <div class="text-xl font-extrabold">{{ formatCents(monthOut) }}</div>
            </div>
        </div>

        <!-- Combined history, one section per month that has rows. Every month past the
             first is one the server named because it holds something, so the only empty
             page is normally the current month, and a header over nothing is noise. -->
        <section v-for="page in months" :key="page.month" class="flex flex-col gap-2.5">
            <header class="flex items-baseline justify-between">
                <h2 class="text-sm font-extrabold">{{ monthLabel(page.month) }}</h2>
                <span class="text-xs font-semibold text-muted-foreground">{{ net(page) }} net</span>
            </header>

            <ul class="overflow-hidden rounded-md border border-border bg-card">
                <li v-for="tx in page.transactions" :key="tx.id"
                    class="flex items-center gap-3 border-b border-border px-3 py-2 last:border-b-0">
                    <div class="flex size-9 items-center justify-center rounded-md" :class="kindStyle[tx.kind].chip">
                        <component :is="kindStyle[tx.kind].icon" class="size-5" />
                    </div>

                    <div class="flex-1">
                        <div class="text-sm font-semibold">{{ title(tx) }}</div>
                        <div class="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                            {{ sub(tx) }}
                            <div v-if="tx.pending"
                                class="rounded-md bg-accent px-1.5 py-0.5 text-xs font-bold text-primary">
                                PENDING
                            </div>
                        </div>
                    </div>

                    <span class="text-sm font-bold" :class="tx.amountCents >= 0 ? 'text-success' : ''">
                        {{ signed(tx.amountCents) }}
                    </span>
                </li>
            </ul>
        </section>

        <div v-if="isPending" class="py-6 text-center text-sm text-muted-foreground font-medium">
            Loading…
        </div>
        <!-- `hasNextPage` because a quiet current month is not an empty history: there
             are older months to come, and saying "nothing" over them is a lie. -->
        <div v-else-if="!hasNextPage && !months.length"
            class="py-6 text-center text-sm text-muted-foreground font-medium">
            Nothing has moved yet. Bookings you make and money you earn show up here.
        </div>

        <!-- Crossing this asks for the next month. It sits inside the scrolling page
             rather than at a fixed offset, so it fires exactly once per month. -->
        <div ref="sentinel" class="h-px"></div>
        <div v-if="isFetchingNextPage" class="pb-4 text-center text-sm text-muted-foreground font-medium">
            Loading…
        </div>
    </div>
</template>