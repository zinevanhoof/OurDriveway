-- spot-service's database. Private to its service; see migrations/user/0001_init.sql
-- for the shared reasoning about _lease, _outbox and why there is no row-level
-- security here.

CREATE TABLE spot (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,

    -- A plain uuid, not a link. The user table lives in another service's database
    -- and could not be dereferenced from here even when links existed.
    host_id       uuid        NOT NULL,
    title          text        NOT NULL,
    description    text,

    -- EUR cents. Never a float: binary floating point cannot represent most decimal
    -- prices exactly, and this feeds what a renter is actually charged.
    price_per_hour bigint      NOT NULL,
    images         text[]      NOT NULL DEFAULT '{}',

    -- Was `geometry<point>`. PostGIS is not available on YSQL (no GiST), so the point
    -- is two columns and distance is computed rather than indexed as a geometry.
    -- `spot_bbox` below is what makes the radius query an index scan; the exact
    -- haversine runs over what that returns. In practice this is FASTER than what it
    -- replaces — fn::spot_distance was a function call per row with no spatial index
    -- at all.
    --
    -- Order is lng, lat everywhere in this codebase, matching geo's x/y.
    lng            double precision NOT NULL,
    lat            double precision NOT NULL,

    -- Two different "off" states. `active = false` is the host's live switch: no new
    -- reservations, bookings already taken are still honoured. `deleted` is permanent
    -- — the row survives only so a renter's past bookings can still resolve a title
    -- and an address, and every list filters it out.
    active         boolean     NOT NULL DEFAULT true,
    deleted        boolean     NOT NULL DEFAULT false,

    -- Was seven scalar columns and a nested object respectively. jsonb, because
    -- nothing in any query reaches into them — they are read and written whole as
    -- Rust values (general_models::spot::{Address, Availability}) and the grid's own
    -- rules are enforced by garde on the way in, not by the database.
    address        jsonb       NOT NULL,
    availability   jsonb       NOT NULL,

    timezone       text        NOT NULL,

    -- Written by Rust, never by a database clock. Every replica applying the same
    -- event must derive the same timestamp, and `now()` here would differ per
    -- instance. (In SurrealDB `VALUE time::now()` was also a plain bug: it
    -- re-evaluated on UPDATE, so created_at tracked the last write.)
    created_at     timestamptz NOT NULL,
    updated_at     timestamptz NOT NULL
);

CREATE INDEX spot_host ON spot (host_id);

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
