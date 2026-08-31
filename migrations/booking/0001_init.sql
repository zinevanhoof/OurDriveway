-- booking-service's database. Private to its service.

-- ─── booking ────────────────────────────────────────────────────────────────
-- spot_id / owner_id are plain uuids: the authoritative spot lives in spot-service's
-- database. owner_id is denormalized in at create time so a booking can be scoped to
-- the host without a cross-database dereference; renter_id comes from the JWT.
CREATE TABLE booking (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,

    spot_id        uuid        NOT NULL,
    owner_id       uuid        NOT NULL,
    renter_id      uuid        NOT NULL,

    -- One or more dates, each with one or more slots: {"YYYY-MM-DD": [{start,end}]},
    -- as "HH:MM" strings in the SPOT's timezone. jsonb for the same reason as
    -- spot.availability — read and written whole, never queried into. `ends_at` below
    -- is the folded, queryable half.
    booked         jsonb       NOT NULL DEFAULT '{}'::jsonb,

    -- EUR cents. Matches spot.price_per_hour and view's booking.amount.
    amount         bigint      NOT NULL,

    -- 'reserved' is a hold taken while checkout runs: it blocks slots exactly like a
    -- confirmed booking, and ends as 'confirmed' (paid) or 'released' (the renter
    -- backed out, or the hold lapsed and the sweeper collected it).
    --
    -- The CHECK is kept where the email one was dropped, and the difference is
    -- deliberate: an unknown status is a real bug, and failing loudly is the correct
    -- response. `transition()` guards on these exact values.
    status         text        NOT NULL DEFAULT 'reserved'
                   CHECK (status IN ('reserved','confirmed','released','completed','cancelled')),

    -- When a hold lapses. THE only place a hold expiry is stored, and deliberately not
    -- part of what makes a booking block a slot: a 'reserved' row blocks, full stop. A
    -- lapsed hold stops blocking when the sweeper publishes Released, not because a
    -- reader learned to compare this against the clock.
    hold_until     timestamptz,

    -- Two reasons, not one. `release_reason` says why a HOLD ended; `cancel_reason`
    -- says who withdrew a PAID booking. Only ever one is set, but collapsing them
    -- would leave a renter's history unable to tell "your hold ran out" from "the host
    -- pulled the listing".
    release_reason text        CHECK (release_reason IS NULL OR release_reason IN ('abandoned','expired')),
    cancel_reason  text        CHECK (cancel_reason  IS NULL OR cancel_reason  IN ('by_renter','spot_unavailable')),

    -- The last moment this booking occupies, as an instant. `booked` holds wall-clock
    -- strings in the spot's zone, so "is it over yet" cannot be asked of it in a query
    -- without knowing that zone — the folded answer is stored instead.
    ends_at        timestamptz NOT NULL,

    rating         integer     CHECK (rating IS NULL OR rating BETWEEN 1 AND 5),
    created_at     timestamptz NOT NULL
);

CREATE INDEX booking_renter ON booking (renter_id);

-- The sweeper's query: status = 'reserved' AND hold_until < now().
CREATE INDEX booking_hold ON booking (status, hold_until);

-- Both per-spot queries: reserve's "what blocks these times" and the cancel reactor's
-- "what is still to come". Both are (spot_id =, ends_at >) with status as a residual
-- filter — status deliberately does NOT lead, because reserve asks `status NOT IN`,
-- which cannot range-scan. A plain spot_id lookup is served by this index's prefix.
CREATE INDEX booking_spot ON booking (spot_id, ends_at);

-- ─── spot (local projection of the SPOTS stream) ────────────────────────────
-- booking-service needs price, availability and timezone to authorize and price a
-- booking server-side, and the authoritative spot lives in another service's database.
--
-- The SPOTS-owned columns are NULLABLE on purpose. The two projectors advance
-- independently, so on a cold rebuild a BOOKINGS event can arrive for a spot whose
-- SpotCreated has not been applied yet and create this row first. Reserve rejects a
-- spot whose availability is still absent, so the gap fails CLOSED rather than
-- booking against nothing.
CREATE TABLE spot (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,

    owner_id       uuid,
    price_per_hour bigint,
    timezone       text,
    availability   jsonb,

    active         boolean     NOT NULL DEFAULT true,
    -- Withdrawn for good. Reserve already refuses `active = false`; this additionally
    -- stops a hold taken BEFORE the deletion from being confirmed, because the host
    -- has said they cannot provide the space at all.
    deleted        boolean     NOT NULL DEFAULT false
);

-- `bookings_seq` is deliberately absent, and its removal is the point of the whole
-- Read Committed design. It was a synthetic collision key: two renters racing one slot
-- insert two DIFFERENT booking rows — different keys, nothing collides — so the code
-- bumped a shared counter to force TiKV to refuse one of them.
--
-- Under Read Committed that bump would not conflict at all (the second writer blocks,
-- re-reads and applies), so the counter would have silently stopped working. What
-- replaces it is a real lock: reserve takes `SELECT … FROM spot WHERE id = $1 FOR
-- UPDATE` on THIS row, and Read Committed then gives its next statement a fresh
-- snapshot containing the winner's booking — so the loser sees the taken slot and
-- answers a clean 409 instead of retrying.
--
-- Measured on yugabytedb/yugabyte:2025.2.5.2-b5 before any of this was written:
-- without the FOR UPDATE two racers both see zero bookings and both insert.

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
