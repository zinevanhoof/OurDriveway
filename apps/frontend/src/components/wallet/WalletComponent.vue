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
import { Badge } from '@/components/ui/badge'
import { Surface } from '@/components/base/surface'
import { Text, Title } from '@/components/base/text'
import { IconBox } from '@/components/base/icon-box'
import { Money } from '@/components/base/money'
import { SectionHeader } from '@/components/base/section-header'
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
    in: { icon: Banknote, tone: 'brand' },
    refund: { icon: RotateCcw, tone: 'brand' },
    out: { icon: CarFront, tone: 'muted' },
    payout: { icon: Landmark, tone: 'muted' }
} as const

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
    const cents = page.inCents - page.outCents
    return `${cents >= 0 ? '+' : '−'}${formatCents(Math.abs(cents))}`
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
        <Title as="h1" size="xl" weight="extrabold">Wallet</Title>

        <!-- Balance -->
        <Surface variant="primary" size="lg" class="gap-4">
            <div>
                <Text size="eyebrow" weight="semibold" tone="inverse" class="opacity-80">Available to withdraw</Text>
                <Money :cents="available" tone="inverse" weight="extrabold" class="text-[34px] leading-none" />
            </div>

            <Surface v-if="pending > 0" variant="none" size="sm" orientation="horizontal" class="gap-2 bg-white/[0.14]">
                <Clock class="size-4 shrink-0 opacity-90" />
                <Text as="span" size="xs" weight="semibold" tone="inverse" class="leading-[1.35] opacity-95">
                    {{ formatCents(pending) }} pending — clears 24h after each booking ends
                </Text>
            </Surface>

            <Button class="h-11 bg-primary-foreground text-primary font-bold"
                @click="router.push({ name: 'wallet-withdraw', params: { maxWithdraw: available } })">
                <ArrowUpRight class="size-4.5" />
                Withdraw
            </Button>
        </Surface>

        <!-- Month totals -->
        <div class="grid grid-cols-2 gap-2">
            <Surface size="sm">
                <Text size="xs" weight="bold" tone="default">MONEY IN · {{ currentMonthShort }}</Text>
                <Money :cents="monthIn" size="xl" weight="extrabold" tone="success" />
            </Surface>
            <Surface size="sm">
                <Text size="xs" weight="bold" tone="default">MONEY OUT · {{ currentMonthShort }}</Text>
                <Money :cents="monthOut" size="xl" weight="extrabold" />
            </Surface>
        </div>

        <!-- Combined history, one section per month that has rows. Every month past the
             first is one the server named because it holds something, so the only empty
             page is normally the current month, and a header over nothing is noise. -->
        <section v-for="page in months" :key="page.month" class="space-y-2.5">
            <SectionHeader as="header" class="items-baseline">
                <Title as="h2" size="sm" weight="extrabold">{{ monthLabel(page.month) }}</Title>
                <template #action>
                    <Text as="span" size="xs" weight="semibold">{{ net(page) }} net</Text>
                </template>
            </SectionHeader>

            <Surface as="ul" size="none" class="overflow-hidden">
                <Surface v-for="tx in page.transactions" :key="tx.id" as="li" variant="none" size="sm"
                    orientation="horizontal" class="gap-3 border-b border-border rounded-none last:border-b-0">
                    <IconBox :tone="kindStyle[tx.kind].tone">
                        <component :is="kindStyle[tx.kind].icon" />
                    </IconBox>

                    <div class="flex-1">
                        <Title size="sm" weight="semibold">{{ title(tx) }}</Title>
                        <Text class="flex items-center gap-1.5">
                            {{ sub(tx) }}
                            <Badge v-if="tx.pending" class="bg-accent px-1.5 text-primary">
                                PENDING
                            </Badge>
                        </Text>
                    </div>

                    <Money :cents="tx.amountCents" signed size="sm" weight="bold" />
                </Surface>
            </Surface>
        </section>

        <Text v-if="isPending" size="sm" class="py-6 text-center">
            Loading…
        </Text>
        <!-- `hasNextPage` because a quiet current month is not an empty history: there
             are older months to come, and saying "nothing" over them is a lie. -->
        <Text v-else-if="!hasNextPage && !months.length" size="sm" class="py-6 text-center">
            Nothing has moved yet. Bookings you make and money you earn show up here.
        </Text>

        <!-- Crossing this asks for the next month. It sits inside the scrolling page
             rather than at a fixed offset, so it fires exactly once per month. -->
        <div ref="sentinel" class="h-px"></div>
        <Text v-if="isFetchingNextPage" size="sm" class="pb-4 text-center">
            Loading…
        </Text>
    </div>
</template>