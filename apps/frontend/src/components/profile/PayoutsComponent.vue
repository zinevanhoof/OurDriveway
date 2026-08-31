<script setup lang="ts">
/**
 * A host's earnings and the withdraw button.
 *
 * Nothing here moves real money. There is no Stripe Connect account, no transfer and no
 * bank — a payout is a row that reduces the balance, so the demo has a complete loop.
 * The UI says so rather than implying otherwise.
 *
 * Every figure comes from payment-service. The history list comes from view-service.
 * That split is deliberate: a total derived one way, next to a button that spends a
 * total derived another way, is how the two end up disagreeing.
 */
import { computed, onMounted, ref } from 'vue';
import { useQuery, useQueryClient } from '@tanstack/vue-query';
import { toast } from 'vue-sonner';
import { Banknote, Wallet } from '@lucide/vue';
import Button from '@/components/ui/button/Button.vue';
import Separator from '@/components/ui/separator/Separator.vue';
import Spinner from '@/components/ui/spinner/Spinner.vue';
import { formatCents } from '@/lib/money';
import { fetchPayouts, viewKeys } from '@/api/viewApi';
import * as paymentApi from '@/api/paymentApi';

const queryClient = useQueryClient();

const earnings = ref<paymentApi.Earnings | null>(null);
const loading = ref(true);
const busy = ref(false);

// No `ownerId` variable and no `pause` on it. The server takes the owner from the
// verified token, so there is no id to wait for the auth store to supply — which is
// what that `pause` was guarding against.
const { data } = useQuery({
    queryKey: viewKeys.payouts,
    queryFn: fetchPayouts,
});

// Already newest-first from the server (`payout_owner (owner_id, created_at)` serves
// the ORDER BY), so the client-side sort this replaced is gone.
const payouts = computed(() => data.value ?? []);

const df = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });

async function load() {
    try {
        earnings.value = await paymentApi.earnings();
    } catch (e: any) {
        toast.error("Couldn't load your earnings", { description: e.message });
    } finally {
        loading.value = false;
    }
}

onMounted(load);

async function withdraw() {
    if (busy.value) return;
    busy.value = true;
    try {
        // No amount is sent — the server computes it from settled earnings minus prior
        // payouts, so there is nothing here a caller could inflate.
        const amount = await paymentApi.requestPayout();
        toast.success('Withdrawn', {
            description: `${formatCents(amount)} on its way. Demo only — no money actually moved.`,
        });
        // Re-read both: the balance from payment-service, the list from the projection.
        // The list may lag by a moment, which invalidating does not fix and does not
        // need to — the next visit picks it up.
        await load();
        void queryClient.invalidateQueries({ queryKey: viewKeys.payouts });
    } catch (e: any) {
        toast.error("Couldn't withdraw", { description: e.message });
    } finally {
        busy.value = false;
    }
}
</script>

<template>
    <div class="bg-card border border-border rounded-md">
        <div class="flex gap-3 items-center p-3">
            <div class="p-2 rounded-md bg-accent text-accent-foreground">
                <Wallet :size="18" />
            </div>
            <div class="flex-1">
                <div class="text-xs text-muted-foreground font-medium">Available to withdraw</div>
                <div class="text-xl font-bold">
                    <Spinner v-if="loading" class="size-4" />
                    <template v-else>{{ formatCents(earnings?.availableCents ?? 0) }}</template>
                </div>
            </div>
        </div>

        <template v-if="earnings">
            <Separator />
            <div class="grid grid-cols-2 gap-3 p-3 text-sm">
                <div>
                    <div class="text-xs text-muted-foreground font-medium">Earned</div>
                    <div class="font-bold">{{ formatCents(earnings.earnedCents) }}</div>
                </div>
                <div>
                    <div class="text-xs text-muted-foreground font-medium">Withdrawn</div>
                    <div class="font-bold">{{ formatCents(earnings.paidOutCents) }}</div>
                </div>
            </div>
        </template>

        <Separator />
        <div class="p-3 space-y-2">
            <Button class="w-full h-11 font-bold" :disabled="busy || loading || !earnings?.availableCents"
                @click="withdraw">
                <Banknote />
                Withdraw {{ formatCents(earnings?.availableCents ?? 0) }}
            </Button>
            <!-- Said plainly rather than buried. A withdraw button that looks real and
                 isn't is worse than one that admits it. -->
            <div class="text-xs text-muted-foreground font-medium text-center">
                Demo only — no money leaves or enters any account.
            </div>
            <div v-if="!loading && !earnings?.availableCents"
                class="text-xs text-muted-foreground font-medium text-center">
                Earnings become available a day after a booking ends.
            </div>
        </div>

        <template v-if="payouts.length">
            <Separator />
            <div class="p-3 space-y-1.5">
                <div class="text-xs text-muted-foreground font-bold">Withdrawal history</div>
                <div v-for="p in payouts" :key="p.id"
                    class="flex items-center justify-between text-sm font-semibold">
                    <span class="text-muted-foreground">{{ df.format(new Date(p.createdAt)) }}</span>
                    <span>{{ formatCents(p.amount) }}</span>
                </div>
            </div>
        </template>
    </div>
</template>
