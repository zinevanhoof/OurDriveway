-- payment-service's database. Private to its service, more strictly than any other:
-- it holds Stripe intent ids and what every renter was charged. Client reads of money
-- go through payment-service's own handlers, which scope by the verified JWT claim.
-- Payout HISTORY is readable by hosts, but from view-service's projection, not here.

-- ─── payment ────────────────────────────────────────────────────────────────
-- One row per booking that someone started paying for.
CREATE TABLE payment (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,

    booking_id     uuid        NOT NULL,
    -- Who earns it and who paid, both denormalized off the event so the earnings
    -- query never dereferences a booking.
    host_id       uuid        NOT NULL,
    renter_id      uuid        NOT NULL,

    -- EUR cents, as the server priced the booking at reserve time. Never a float:
    -- this is the figure actually charged.
    amount_cents   bigint      NOT NULL,

    -- The two Stripe handles, split by when each becomes knowable. Neither is an
    -- identifier — this row is found by booking_id, and a webhook finds its way home
    -- via the intent's metadata. These are only ever arguments to a Stripe call.
    --
    -- `cs_…`, known the moment the session is created. This is what expires an unpaid
    -- checkout, and it has to be the session because at that point no intent exists.
    session_id     text        NOT NULL,
    -- `pi_…`, NULL until the payment succeeds — a Checkout Session has no
    -- PaymentIntent until it is paid. This is what a refund is issued against, because
    -- CreateRefund accepts an intent or a charge and never a session.
    --
    -- Written in the SAME statement as status = 'succeeded', which is the invariant
    -- the refund path leans on: settle_up's Refund arm only matches 'succeeded', so it
    -- cannot be reached while this is NULL. Keep the two writes together.
    intent_id      text,

    -- 'created' means a session exists and nothing has been charged. The three
    -- terminal states are 'succeeded', 'refunded' and 'expired'. 'failed' is NOT
    -- terminal: the renter may confirm the same session again with another method,
    -- which is why it does not stop settle_up expiring it later.
    status         text        NOT NULL
                   CHECK (status IN ('created','succeeded','failed','refunded','expired')),

    -- Set together with status = 'refunded'. Its presence is what makes the refund
    -- path idempotent — settle_up refuses a payment that already has one, so a
    -- redelivered Cancelled cannot refund twice.
    refund_id      text,
    -- Stripe's own message from a failed attempt, kept for the log. Never rendered to
    -- a renter: the Payment Element already told them, in their language.
    failure_reason text,
    created_at     timestamptz NOT NULL
);

-- UNIQUE, and load-bearing. One booking gets at most one PaymentIntent: a failed
-- attempt is retried on the SAME intent rather than a new one, so a second row for one
-- booking means two intents exist and the renter could be charged twice. Anything that
-- trips this is a real bug, and failing the projector (readiness 503) beats quietly
-- building a database that can double-charge.
CREATE UNIQUE INDEX payment_booking ON payment (booking_id);

-- The checkout screen's lookup: it knows only a session id, the whole handle it
-- carries in its URL. UNIQUE for the same reason as payment_booking.
CREATE UNIQUE INDEX payment_session ON payment (session_id);

-- The earnings query: sum by host over succeeded payments.
CREATE INDEX payment_host ON payment (host_id, status);

-- ─── booking (local projection of the BOOKINGS stream) ──────────────────────
-- Unlike booking-service's spot mirror, nothing here is nullable: BookingCreated is
-- always the first event for a booking and BOOKINGS never expires, so a replay can
-- only ever create this row complete.
CREATE TABLE booking (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,

    spot_id        uuid        NOT NULL,
    host_id       uuid        NOT NULL,
    renter_id      uuid        NOT NULL,
    amount_cents   bigint      NOT NULL,

    -- Projected for ONE reason: the Checkout Session's line item, which reads
    -- "Parking · 14 Aug, 09:00–11:00". That is the only description of what is being
    -- bought that ever reaches the renter's payment screen or their Stripe receipt.
    -- Bare wall-clock strings in the spot's zone, rendered literally — which is why
    -- this service still needs no timezone.
    booked         jsonb       NOT NULL DEFAULT '{}'::jsonb,

    -- 'completed' is deliberately absent, unlike booking-service's list. Nothing sets
    -- it, and this service does not need it: "the host has earned this" is `confirmed`
    -- plus an ends_at far enough in the past. Add it here only if booking-service ever
    -- publishes it, or this projector stops on an unknown status — the correct failure.
    status         text        NOT NULL
                   CHECK (status IN ('reserved','confirmed','released','cancelled')),

    -- NULL once the booking is no longer 'reserved'. Read when creating a payment: an
    -- expired hold must not be payable.
    hold_until     timestamptz,

    -- The field the settlement window is measured against, which is why it is
    -- projected rather than recomputed from `booked` — that map is wall-clock strings
    -- in the spot's timezone and cannot be compared to now() without the zone.
    ends_at        timestamptz NOT NULL,

    cancel_reason  text,
    release_reason text
);

-- The earnings query's half of the join: a host's bookings that are confirmed and old
-- enough to have settled.
CREATE INDEX booking_host ON booking (host_id, status, ends_at);

-- ─── payout ─────────────────────────────────────────────────────────────────
-- A host withdrawing their balance. NOTHING REAL MOVES — no Stripe Connect account, no
-- transfer, no bank. The row exists only so the balance goes down and stays down
-- across a reload.
--
-- There is deliberately no stored balance anywhere. Available is always
-- `earnings − Σ payout.amount_cents`, computed on read, which is what makes a refunded
-- booking drop out of a host's income for free instead of needing a compensating write.
CREATE TABLE payout (
    id           uuid PRIMARY KEY,
    version      bigint      NOT NULL DEFAULT 0,
    host_id     uuid        NOT NULL,
    amount_cents bigint      NOT NULL,
    created_at   timestamptz NOT NULL
);

CREATE INDEX payout_host ON payout (host_id);

-- ─── how two concurrent withdrawals are stopped ─────────────────────────────
-- There is no `host` table, and its absence is deliberate enough to be worth the
-- paragraph.
--
-- A balance is derived (`earnings − Σ payout.amount_cents`), never stored, so two
-- double-clicked withdrawals both read the same available amount and both insert a
-- DIFFERENT payout row — different keys, nothing collides, money out twice. Under TiKV
-- a one-column `host` table was bumped inside the transaction purely to manufacture a
-- write conflict. Under Read Committed that bump would not conflict at all (the second
-- writer blocks, re-reads and applies), so it had to be replaced rather than ported.
--
-- Locking the payout rows themselves does NOT work: the row that changes the answer is
-- one that does not exist yet, and a first-time withdrawer has zero rows to lock. That
-- phantom is exactly what Read Committed permits, and it is the reason a stand-in row
-- was needed at all.
--
-- So the lock is taken on the HOST ID rather than on a row invented to hold it:
--
--   BEGIN;
--   SELECT pg_advisory_xact_lock($1);   -- $1 = low 64 bits of host_id
--   -- the balance query MUST come after this, never before. That is the second half
--   -- of the fix and it is easy to miss: available_for() used to run before BEGIN, so
--   -- the loser's balance was already stale no matter what it locked afterwards.
--   -- Read Committed gives this statement a fresh snapshot containing the winner's
--   -- payout, so the loser computes 0 and answers the clean 422 the endpoint already
--   -- has for "nothing to withdraw".
--   INSERT INTO payout …;
--   COMMIT;
--
-- `pg_advisory_xact_lock` and not the session-scoped variant: it is released by COMMIT
-- or ROLLBACK, so there is no unlock to forget on the `?` early-returns this handler is
-- full of — and no Drop that would have to await one.
--
-- Verified on yugabytedb/yugabyte:2025.2.5.2-b5: two sessions on the same key, the
-- second blocks and then reads the first's committed payout; on different keys neither
-- waits, so hosts do not serialise against each other.

-- ─── leader election ────────────────────────────────────────────────────────
CREATE TABLE _lease (
    name       text PRIMARY KEY,
    holder     text        NOT NULL,
    expires_at timestamptz NOT NULL
);

-- ─── transactional outbox ───────────────────────────────────────────────────
CREATE TABLE _outbox (
    id         uuid PRIMARY KEY,
    subject    text        NOT NULL,
    payload    text        NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX _outbox_order ON _outbox (created_at ASC, id ASC);
