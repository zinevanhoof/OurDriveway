-- One database per service inside the shared `main` namespace. Was DB main for
-- every service, which was safe only while each ran its own SurrealDB over its
-- own datastore; on TiKV they share one keyspace and this name is the isolation.
USE NS main DB payment;

-- This database is PRIVATE to its service, more strictly than any of the others: it
-- holds Stripe intent ids and what every renter was charged. No GraphQL config and no
-- record access are defined, so a client JWT cannot authenticate against it at all,
-- and every table below is additionally PERMISSIONS NONE. Client reads of money go
-- through payment-service's own handlers, which scope by the verified JWT claim —
-- never by a database permission clause on a connection a browser could hold.
--
-- Payout *history* is readable by hosts, but from view-service's projection, not here.

-- Every DEFINE is OVERWRITE: the datastore is persistent in dev (surrealkv) and
-- disposable in k8s (emptyDir), and --import-file re-runs this file on every boot in
-- both. Without OVERWRITE the second boot fails on redefinition.

-- ─── payment ────────────────────────────────────────────────────────────────
-- One row per booking that someone started paying for. Written only by the PAYMENTS
-- projector, from events payment-service itself published.
DEFINE TABLE OVERWRITE payment SCHEMAFULL PERMISSIONS NONE;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`payment:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON payment TYPE int DEFAULT 0;

-- The booking this pays for, as a plain string: the booking table below is a local
-- projection, and the authoritative one lives in booking-service's database, so this
-- can't be a record<> link that means anything.
DEFINE FIELD OVERWRITE booking_id ON payment TYPE uuid;

-- `"user:abc"` for both. owner_id is who earns it and renter_id is who paid, both
-- denormalized off the event so the earnings query never dereferences a booking.
DEFINE FIELD OVERWRITE owner_id  ON payment TYPE uuid;
DEFINE FIELD OVERWRITE renter_id ON payment TYPE uuid;

-- EUR cents, as the server priced the booking at reserve time. Matches
-- booking.amount and view-schema's booking.amount; never a float, because binary
-- floating point cannot represent most decimal prices exactly and this is the figure
-- actually charged.
DEFINE FIELD OVERWRITE amount_cents ON payment TYPE int;

-- The two Stripe handles, split by when each becomes knowable. Neither is an
-- identifier: this row is found by `booking_id`, and a webhook finds its way home via
-- the intent's `metadata.booking_id`. These are only ever arguments to a Stripe call.

-- `cs_…`, known the moment the session is created. This is what expires an unpaid
-- checkout, and it has to be the session because at that point no intent exists.
DEFINE FIELD OVERWRITE session_id ON payment TYPE string;

-- `pi_…`, NONE until the payment succeeds — a Checkout Session has no PaymentIntent
-- until it is paid. This is what a refund is issued against, because `CreateRefund`
-- accepts an intent or a charge and never a session.
--
-- Written in the *same statement* as `status = 'succeeded'`, which is the invariant the
-- refund path leans on: `settle_up`'s Refund arm only matches 'succeeded', so it cannot
-- be reached while this is NONE. Keep the two writes together if this is ever touched.
DEFINE FIELD OVERWRITE intent_id ON payment TYPE option<string>;

-- 'created' means a session exists and nothing has been charged. The three terminal
-- states are 'succeeded' (money taken and the booking confirmed), 'refunded' (taken
-- and given back) and 'expired' (the session was voided before anyone paid).
-- 'failed' is NOT terminal: the renter may confirm the same session again with another
-- method, which is why it does not stop `settle_up` from expiring it later.
DEFINE FIELD OVERWRITE status ON payment TYPE string
    ASSERT $value IN ['created', 'succeeded', 'failed', 'refunded', 'expired'];

-- Set together with status = 'refunded'. Its presence is what makes the refund path
-- idempotent — `settle_up` refuses a payment that already has one, so a redelivered
-- Cancelled cannot refund twice.
DEFINE FIELD OVERWRITE refund_id ON payment TYPE option<string>;

-- Stripe's own message from a failed attempt, kept for the log. Never rendered to a
-- renter: the Payment Element already told them, in their language.
DEFINE FIELD OVERWRITE failure_reason ON payment TYPE option<string>;

-- No `VALUE time::now()` — see the note in spot-schema.surql. Comes from the event,
-- because every replica must derive the same timestamp.
DEFINE FIELD OVERWRITE created_at ON payment TYPE datetime;

-- UNIQUE, and load-bearing. One booking gets at most one PaymentIntent: a failed
-- attempt is retried on the *same* intent rather than a new one, so a second row for
-- one booking means two intents exist and the renter could be charged twice. The
-- projector upserts by payment_id, so a redelivery can't trip this — anything that
-- does trip it is a real bug, and failing the projector here stops the instance
-- (readiness 503) instead of quietly building a database that can double-charge.
DEFINE INDEX OVERWRITE payment_booking ON payment FIELDS booking_id UNIQUE;
-- The earnings query: sum by host over succeeded payments.
DEFINE INDEX OVERWRITE payment_owner   ON payment FIELDS owner_id, status;
-- The checkout screen's lookup. It knows only a session id — that is the whole handle it
-- carries in its URL — and this is what turns that into a row we can authorize against
-- before asking Stripe anything. UNIQUE for the same reason `payment_booking` is: one
-- session per payment, so two rows claiming one session would be a real bug.
DEFINE INDEX OVERWRITE payment_session ON payment FIELDS session_id UNIQUE;

-- ─── booking (local projection of the BOOKINGS stream) ──────────────────────
-- payment-service needs a booking's price, renter, host and lifecycle to authorize a
-- payment and to decide a refund, and the booking table lives in another service's
-- database.
--
-- Unlike booking-service's spot projection, nothing here is option<>: BookingCreated
-- is always the first event for a booking and BOOKINGS never expires (max_age None),
-- so a replay from sequence 1 can only ever create this row complete.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS NONE;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`booking:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON booking TYPE int DEFAULT 0;

DEFINE FIELD OVERWRITE spot_id    ON booking TYPE uuid;
DEFINE FIELD OVERWRITE owner_id   ON booking TYPE uuid;
DEFINE FIELD OVERWRITE renter_id  ON booking TYPE uuid;
DEFINE FIELD OVERWRITE amount_cents ON booking TYPE int;

-- The slots booked: "YYYY-MM-DD" -> [{ start, end }], same shape as booking-schema.
--
-- Projected for one reason: the Checkout Session's line item. It reads
-- "Parking · 14 Aug, 09:00–11:00", which is the only description of what is being bought
-- that ever reaches the renter's payment screen or their Stripe receipt — the checkout
-- view renders it straight from `getSession()` rather than refetching the booking.
--
-- These are bare wall-clock strings in the spot's zone and are rendered literally, so
-- this service still needs no timezone. It is the spot's local time, which is exactly
-- what a renter wants to read.
DEFINE FIELD OVERWRITE booked           ON booking TYPE object DEFAULT {};
DEFINE FIELD OVERWRITE booked.*         ON booking TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON booking TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON booking TYPE string;

-- 'completed' is deliberately absent, unlike booking-schema's list. Nothing in the
-- system ever sets it, and this service does not need it: "the host has earned this"
-- is `confirmed` plus an `ends_at` far enough in the past (see SETTLEMENT_SECS in
-- payment-service's Config). Add it here only if booking-service ever publishes it,
-- or this projector will stop on an unknown status — which is the correct failure.
DEFINE FIELD OVERWRITE status ON booking TYPE string
    ASSERT $value IN ['reserved', 'confirmed', 'released', 'cancelled'];

-- When the hold lapses. NONE once the booking is no longer 'reserved'. Read when
-- creating a payment: an expired hold must not be payable.
DEFINE FIELD OVERWRITE hold_until ON booking TYPE option<datetime>;

-- The last moment the booking occupies, as an instant. This is the field the
-- settlement window is measured against, which is why it is projected rather than
-- recomputed from `booked` — that map is wall-clock strings in the spot's timezone
-- and cannot be compared to `now` without knowing the zone.
DEFINE FIELD OVERWRITE ends_at ON booking TYPE datetime;

-- Why the booking was withdrawn, when it was. Carried so a refund policy can one day
-- distinguish a host cancelling from a renter cancelling — see the doc comment on
-- CancelReason. Today every refund is the full amount and this is written but unused.
DEFINE FIELD OVERWRITE cancel_reason  ON booking TYPE option<string>;
DEFINE FIELD OVERWRITE release_reason ON booking TYPE option<string>;

-- The earnings query's half of the join: a host's bookings that are confirmed and
-- old enough to have settled.
DEFINE INDEX OVERWRITE booking_owner ON booking FIELDS owner_id, status, ends_at;

-- ─── payout ─────────────────────────────────────────────────────────────────
-- A host withdrawing their balance. **Nothing real moves** — no Stripe Connect
-- account, no transfer, no bank. This is the demo of a payout, and the row exists
-- only so the balance goes down and stays down across a reload.
--
-- There is deliberately no stored balance anywhere. Available is always
-- `earnings − Σ payout.amount_cents`, computed on read, which is what makes a
-- refunded booking drop out of a host's income for free instead of needing a
-- compensating write.
DEFINE TABLE OVERWRITE payout SCHEMAFULL PERMISSIONS NONE;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`payout:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON payout TYPE int DEFAULT 0;

DEFINE FIELD OVERWRITE owner_id     ON payout TYPE uuid;
DEFINE FIELD OVERWRITE amount_cents ON payout TYPE int;
DEFINE FIELD OVERWRITE created_at   ON payout TYPE datetime;

DEFINE INDEX OVERWRITE payout_owner ON payout FIELDS owner_id;


-- ─── host ───────────────────────────────────────────────────────────────────
-- One row per host, holding nothing but a version. It exists solely as the key
-- two concurrent withdrawals collide on.
--
-- A host's balance is derived (earnings − payouts), never stored, so two
-- double-clicked withdrawals both read the same available amount and both create
-- a *different* payout row — different keys, no write conflict, money out twice.
-- Bumping this row inside the same transaction is what makes TiKV refuse one of
-- them. It replaced a compare-and-swap on the host's NATS subject, which could not
-- survive the write moving into the database.
DEFINE TABLE OVERWRITE host SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE version ON host TYPE int DEFAULT 0;

-- ─── leader election ────────────────────────────────────────────────────────
-- One row, `_lease:leader`, and it exists for the OUTBOX RELAY only.
--
-- The projectors do not need it: they pull from one durable consumer with
-- `max_ack_pending: 1`, so JetStream itself hands out one event at a time across
-- every replica, in stream order. The relay has no such backstop — it reads rows
-- from this database and publishes them, and two relays could publish one
-- entity's events out of order, which the downstream `WHERE status IN $from`
-- guards drop rather than reorder.
--
-- Expiry is compared against `time::now()` INSIDE the database. Replicas do not
-- share a clock; they do share this row.
DEFINE TABLE OVERWRITE _lease SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE holder     ON _lease TYPE string;
DEFINE FIELD OVERWRITE expires_at ON _lease TYPE datetime;

-- ─── transactional outbox ───────────────────────────────────────────────────
-- Events written in the SAME transaction as the data that caused them, then
-- moved to NATS by the relay. This is the one thing JetStream cannot provide on
-- its own: its guarantees begin once a message is in the stream, and this closes
-- the window before that.
--
-- The record id is the envelope's `event_id`, so enqueuing the same event twice
-- is one row — the same property `Nats-Msg-Id` gives on the way out.
DEFINE TABLE OVERWRITE _outbox SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE subject    ON _outbox TYPE string;
-- The serialized envelope, as text so a stuck row can be read with a SELECT.
DEFINE FIELD OVERWRITE payload    ON _outbox TYPE string;
DEFINE FIELD OVERWRITE created_at ON _outbox TYPE datetime;
-- The relay's read: oldest first. `id` is deliberately NOT in the index — it is
-- the record key, not a field, and SurrealDB rejects it here ("The field 'id'
-- does not exist"). The relay still ORDERs by `created_at, id` so two events
-- enqueued in one transaction, sharing a timestamp, are retried in the order
-- they were sent; only the leading column is indexed.
DEFINE INDEX OVERWRITE _outbox_order ON _outbox FIELDS created_at;
