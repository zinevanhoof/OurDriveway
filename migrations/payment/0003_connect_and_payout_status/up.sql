-- Payouts stop being a bookkeeping entry.
--
-- 0001_init.sql said, in the paragraph over `payout`, that NOTHING REAL MOVES — no
-- Stripe Connect account, no transfer, no bank. That is what this migration reverses.
-- A withdrawal is now a Stripe **Transfer** from the platform balance to the host's
-- connected account, made asynchronously by a worker, so the row grows the two things
-- an asynchronous side effect needs: where it got to, and the handle it produced.
--
-- Charges are unchanged. Money is still taken on the platform account and split only
-- at withdrawal time (Stripe calls this "separate charges and transfers"), which is
-- what keeps the settlement window, refunds and the derived balance exactly as they
-- were — all of that is still ours to compute, not Stripe's.

-- ─── connect_account ────────────────────────────────────────────────────────
-- One row per host who has started onboarding. The Stripe account id stops here, in
-- this service, for the same reason `payment.session_id` and `payment.intent_id` do:
-- these are arguments to a Stripe call, and nothing a browser can reach has any use
-- for them.
--
-- Deliberately NOT a `payouts_enabled` column. Whether a host may be paid is answered
-- by Stripe, live, on the one status call the withdraw page makes — see
-- `ConnectService::status`. A cached copy would need `account.updated` in the webhook
-- to stay true, and a stale `true` is the failure mode that shows a host a withdraw
-- form Stripe will then refuse.
CREATE TABLE connect_account (
    -- The host. PRIMARY KEY, so a second onboarding for the same host is impossible
    -- rather than merely unlikely — two connected accounts for one person would split
    -- their money across two Stripe balances.
    host_id          uuid PRIMARY KEY,
    -- `acct_…`
    stripe_account_id text        NOT NULL UNIQUE,
    created_at        timestamptz NOT NULL
);

-- ─── payout, now with a lifecycle ───────────────────────────────────────────
-- 'requested' is what the request writes; the worker moves it to exactly one of the
-- two terminal states. DEFAULT so the rows written before this migration — which were
-- never anything but bookkeeping — read as requested rather than NULL.
ALTER TABLE payout ADD COLUMN status text NOT NULL DEFAULT 'requested'
      CHECK (status IN ('requested','paid','failed'));

-- `tr_…`, NULL until the transfer succeeds. Not an identifier: this row is found by
-- id, and the handle is only ever an argument to a later Stripe call (a reversal, if
-- one is ever wanted). Its presence is also the second reading of "already paid".
ALTER TABLE payout ADD COLUMN transfer_id    text;

-- Stripe's own message when the transfer was refused. Kept for the log — `failed`
-- rows are filtered out of the wallet, so this is never rendered to a host.
ALTER TABLE payout ADD COLUMN failure_reason text;

-- The worker's lookup is by primary key, so no index is added. `payout_host` from
-- 0001 still serves `total_for`, whose predicate now also names `status` — a filter on
-- three values over one host's handful of rows, which is not worth widening the index
-- for.
--
-- IMPORTANT, and the reason `status` is not merely decorative: `total_for` sums only
-- 'requested' and 'paid'. The balance is derived (`earnings − Σ payouts`), so dropping
-- a failed payout out of that sum IS how the money comes back. There is no
-- compensating write, exactly as with a refunded booking.
