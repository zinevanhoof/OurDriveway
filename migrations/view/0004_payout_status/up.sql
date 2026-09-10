-- Where a withdrawal got to, mirrored from payment-service.
--
-- A payout used to be written once and never touched — 0001_init.sql's `payout` table
-- has no status because there was nothing for it to hold: the row *was* the whole
-- event. It is now a Stripe Transfer made asynchronously by a worker, so there are
-- three states and this projection has to follow all of them.
--
-- The wallet reads it twice, and the two readings must agree with payment-service's
-- `PayoutRepository::total_for`, which is the same arithmetic against a different
-- database:
--
--   'requested'  still in flight  → shown, with the PENDING chip the wallet already has
--   'paid'       done             → shown
--   'failed'     never happened   → filtered out of BOTH the list and the balance
--
-- Filtering a failed payout out of the balance is how the money comes back. Available
-- is `earnings − Σ payouts`, derived on every read, so a row that stops matching is a
-- refund with no compensating write — the same trick a refunded booking already plays.
ALTER TABLE payout ADD COLUMN status text NOT NULL DEFAULT 'requested';

-- No CHECK, for the reason 0003_payment.sql gives over `payment.status`:
-- payment-service's table is the authority on what a status may be, and a constraint
-- here would turn a status it adds into a projector that stops rather than a row the
-- wallet ignores.

-- No index. `payout_host` from 0001 already selects the rows; status is a filter over
-- one host's handful of withdrawals.
