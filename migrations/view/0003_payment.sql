-- ─── payment ────────────────────────────────────────────────────────────────
-- What was charged and what was given back, in the read model.
--
-- THIS REVERSES A RULE. 0001_init.sql and a dozen doc comments said payments must
-- never be projected here, because a second copy of the money would disagree with the
-- balance next to the withdraw button. The split is now the ordinary one: payment-
-- service writes and view-service reads, and every figure a client is shown comes from
-- here. The disagreement that rule was protecting against is prevented where it
-- actually matters instead — `PaymentService::request_payout` computes the amount it
-- pays out inside its own transaction, from its own tables, under an advisory lock. No
-- money is ever *spent* against this projection; it is only ever displayed.
--
-- What it can be is BEHIND, by however far the projector is lagging, which is what the
-- `X-Await-Version` token `POST /api/payment/payout` answers with is for.
CREATE TABLE payment (
    id          uuid PRIMARY KEY,
    version     bigint      NOT NULL DEFAULT 0,
    booking_id  uuid        NOT NULL,
    -- Who earns it and who paid. Both are here rather than reached through the
    -- booking, because the wallet's two halves select on one or the other and an
    -- index cannot be built through a join.
    owner_id    uuid        NOT NULL,
    renter_id   uuid        NOT NULL,
    amount      bigint      NOT NULL,             -- EUR cents
    -- No CHECK. payment-service's own table is the authority on what a status may be,
    -- and a constraint here would turn a status it adds into a projector that stops
    -- rather than a row a wallet ignores.
    status      text        NOT NULL,
    -- When the checkout session was made. The charge is dated at this: the intervening
    -- step is the renter typing a card number, which is minutes, not months.
    created_at  timestamptz NOT NULL,
    -- When the money went back, off the event. NULL unless status = 'refunded', and
    -- also NULL for anything refunded before payment-service grew the column.
    refunded_at timestamptz
);

-- Deliberately NOT the Stripe handles. `session_id`, `intent_id` and `refund_id` are
-- how the write side talks to Stripe; nothing a browser can reach has any use for them,
-- so they stop at payment-service's database.

-- The wallet's two halves, one index each. They are separate statements in a UNION ALL
-- rather than `WHERE owner_id = $1 OR renter_id = $1` precisely so that each can use
-- one of these — and because the sign of the amount differs between the two anyway.
CREATE INDEX payment_owner_created  ON payment (owner_id, created_at);
CREATE INDEX payment_renter_created ON payment (renter_id, created_at);

-- The refund half of the same query, which selects on a different column entirely.
-- Partial: only refunded rows have the column set, and they are a small minority.
CREATE INDEX payment_refunded ON payment (refunded_at) WHERE refunded_at IS NOT NULL;
