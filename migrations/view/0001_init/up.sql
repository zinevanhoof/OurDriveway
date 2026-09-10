-- view-service's database: the combined read model, projected from every service's
-- event stream and serving all client reads.
--
-- ─────────────────────────────────────────────────────────────────────────────
-- WHERE THE SECURITY WENT
--
-- This file used to BE the authorization system. Browsers reached SurrealDB's auto
-- GraphQL directly with their own JWT, and every table carried
-- `PERMISSIONS FOR select WHERE …` plus per-field clauses, evaluated by the database
-- against `$auth`.
--
-- None of that is here, because no browser reaches this database any more. GraphQL is
-- gone; view-service serves REST and holds the only connection. Every rule those
-- clauses expressed now lives in ONE place per table — the repository function that
-- builds that table's SELECT — as an ordinary WHERE over the caller's uuid:
--
--   spot     AND (active OR host_id = $caller)
--   booking  AND (renter_id = $caller OR host_id = $caller
--                 OR status IN ('reserved','confirmed'))
--   payout   AND host_id = $caller
--
-- Field-level scoping (a user's email, a booking's renter/amount/hold_until) is a
-- projection step on the response type. Two things improve by moving:
--
--   * The `option<>`-or-the-whole-array-nulls trap is gone. A denied field used to be
--     CUT from the projected document, and auto GraphQL renders a non-option kind as
--     `T!`, so one null propagated up and blanked the entire bookings array — reading
--     to a client as "nothing is booked".
--   * VULN-001 is gone (see vulns.txt). An equality filter on an INDEXED field was
--     answered from the raw index rather than the reduced document, so
--     `where: {renter_id: {eq: …}}` truthfully answered a question the caller could
--     not read the column for. That oracle existed only because a generic `where` was
--     reachable; there is no generic `where` any more.
--
-- Nothing here is a source of truth. Drop the database and it rebuilds from the log.
-- Never make a decision (availability, pricing, authorization) from data in here —
-- those reads belong to the service that owns them.
--
-- NO FOREIGN KEYS, deliberately. Streams have no cross-stream ordering, so a spot can
-- be projected before the user who owns it. That is the normal case, not an edge one —
-- it is why these were `option<record<user>>` before. The columns are plain uuids and
-- reads LEFT JOIN, so an unresolved reference is an absent join rather than a rejected
-- insert.
-- ─────────────────────────────────────────────────────────────────────────────

-- ─── app_user ───────────────────────────────────────────────────────────────
-- Named for the same reason as in the user database: `user` is reserved, and an
-- unquoted `FROM user` silently reads the current-user keyword instead of erroring.
CREATE TABLE app_user (
    id              uuid PRIMARY KEY,
    version         bigint NOT NULL DEFAULT 0,
    first_name      text   NOT NULL,
    last_name       text   NOT NULL,
    profile_picture text,
    -- The one sensitive column projected here. Every row stays readable — spot-host
    -- profiles have to resolve for everyone — so this is cut per-caller in the
    -- response type, not filtered per-row.
    email           text,
    -- Deliberately public: a host has to recognise the car that turns up on their
    -- driveway, so this is not scoped the way email is.
    license_plates  text[] NOT NULL DEFAULT '{}'
);

-- ─── spot ───────────────────────────────────────────────────────────────────
CREATE TABLE spot (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,
    host_id       uuid        NOT NULL,
    title          text        NOT NULL,
    description    text,
    price_per_hour bigint      NOT NULL,          -- EUR cents
    images         text[]      NOT NULL DEFAULT '{}',
    lng            double precision NOT NULL,
    lat            double precision NOT NULL,
    active         boolean     NOT NULL DEFAULT true,
    -- A deleted spot stays selectable on purpose: a booking still references it, and a
    -- renter's past booking would otherwise render with no title and no address. It is
    -- the LISTS that filter it out.
    deleted        boolean     NOT NULL DEFAULT false,
    address        jsonb       NOT NULL,
    availability   jsonb       NOT NULL,
    timezone       text        NOT NULL,
    created_at     timestamptz NOT NULL,
    updated_at     timestamptz NOT NULL
);

CREATE INDEX spot_host ON spot (host_id);

-- The map's radius query, replacing `fn::spot_distance` + auto GraphQL's `call`
-- filter. That was a function call per row over no spatial index at all; this is a
-- range scan. Partial, because every caller of it filters these two the same way.
--
-- The query is a bounding box on this index, then exact haversine over what comes
-- back — a few hundred rows, not the table.
CREATE INDEX spot_bbox ON spot (lat ASC, lng ASC) WHERE deleted = false AND active;

-- ─── booking ────────────────────────────────────────────────────────────────
-- The renter's and host's view of a booking, AND the source of availability for
-- everyone else — that third role is what replaced the old denormalized `spot.booked`
-- map, and it is why a stranger can select the rows that block a slot.
--
-- Trade-off, stated so it is not rediscovered: booking ids, their slots, their ends_at
-- and reserved-vs-confirmed are enumerable per spot. Renter identity, amounts and hold
-- expiry are not — those are cut per-caller in the response type.
CREATE TABLE booking (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,
    spot_id        uuid        NOT NULL,
    host_id       uuid        NOT NULL,
    renter_id      uuid        NOT NULL,
    booked         jsonb       NOT NULL DEFAULT '{}'::jsonb,
    amount         bigint      NOT NULL,          -- EUR cents
    status         text        NOT NULL
                   CHECK (status IN ('reserved','confirmed','released','completed','cancelled')),
    hold_until     timestamptz,
    release_reason text,
    -- 'spot_unavailable' is how a renter's row can say the HOST withdrew, rather than
    -- showing the same bare "cancelled" they would see for their own doing.
    cancel_reason  text,
    rating         integer     CHECK (rating IS NULL OR rating BETWEEN 1 AND 5),
    -- Folded from `booked` in the spot's zone at reserve time. Upcoming-vs-past is a
    -- filter on this rather than a fold every client repeats.
    ends_at        timestamptz NOT NULL,
    created_at     timestamptz NOT NULL
);

-- The availability query and the host's upcoming list are both (spot_id =, ends_at >),
-- so one composite serves both — and its prefix still serves a plain spot_id lookup.
CREATE INDEX booking_spot ON booking (spot_id, ends_at);
CREATE INDEX booking_renter ON booking (renter_id);

-- The home screen's "earned this week": SUM(amount) WHERE host_id = me AND created_at
-- >= monday. Composite rather than host_id alone — same one index to maintain, but it
-- range-scans the week instead of every booking the host has ever had.
CREATE INDEX booking_host_created ON booking (host_id, created_at);

-- ─── payout ─────────────────────────────────────────────────────────────────
-- A host's withdrawal history, and nothing else about money.
--
-- Payments themselves are deliberately absent. What a renter was charged and what a
-- host has available are payment-service's to answer, and a second copy here would
-- eventually disagree with the balance shown next to the withdraw button.
CREATE TABLE payout (
    id         uuid PRIMARY KEY,
    version    bigint      NOT NULL DEFAULT 0,
    host_id   uuid        NOT NULL,
    amount     bigint      NOT NULL,             -- EUR cents
    created_at timestamptz NOT NULL
);

-- The history list: a host's own withdrawals, newest first.
CREATE INDEX payout_host ON payout (host_id, created_at);

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
