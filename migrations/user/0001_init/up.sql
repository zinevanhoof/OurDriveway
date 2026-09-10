-- user-service's database. Applied by sqlx::migrate! at boot; see shared/src/db.rs.
--
-- This database is PRIVATE to its service. Nothing here is reachable by a browser:
-- client reads are served by view-service from the combined projection, and this
-- connection belongs to user-service alone. There is no row-level security and no
-- per-request identity, because there is no per-request connection — authorization
-- happens in Rust from the verified JWT.

-- ─── app_user ───────────────────────────────────────────────────────────────
-- NOT `user`. That is a reserved word, and the failure is silent rather than loud:
--
--   SELECT count(*) FROM user;    -- returns a row — the KEYWORD current_user
--   SELECT count(*) FROM "user";  -- returns the table
--
-- So a single forgotten quote reads something that is not this table and does not
-- error. Renaming removes the trap instead of relying on never slipping. The
-- AGGREGATE is still "user" — the await token `user:<id>@7`, aggregate_id("user", …)
-- and every event are unchanged; the mapping lives in shared::db's table allowlist.
CREATE TABLE app_user (
    id              uuid PRIMARY KEY,

    -- Monotonic, bumped by the owning service inside the transaction that writes the
    -- row. Two jobs: the token a client waits on (`user:<id>@7`), and the gap
    -- detector that stops an out-of-order event applying silently.
    --
    -- It is NO LONGER the key two concurrent writers collide on. Under Read Committed
    -- a second writer to this row blocks, re-reads and applies rather than being
    -- refused, so the collision has to be made explicit — writers take
    -- `SELECT … FOR UPDATE` on this row first. See shared/src/db.rs.
    version         bigint      NOT NULL DEFAULT 0,

    first_name      text        NOT NULL,
    last_name       text        NOT NULL,

    -- No CHECK for the shape of an address, deliberately, though the SurrealDB schema
    -- had `ASSERT string::is_email($value)`. Requests are validated by garde at the
    -- boundary (shared/src/requests/), and rows are also written by the projector from
    -- events — replaying an event that was legal when it was written must not be
    -- rejected by a rule tightened since. That is the same argument
    -- general_models/spot.rs makes for keeping garde out of Deserialize.
    --
    -- Status CHECKs below are a different case and are kept: an unknown status is a
    -- real bug, and stopping the projector is the correct failure.
    email           text        NOT NULL,

    -- Proven reachable: somebody opened a link only this mailbox received. Login
    -- refuses while this is false, so it is an authentication field and lives only
    -- here — view-service's user projection is world-readable and must not carry it.
    email_verified  boolean     NOT NULL DEFAULT false,
    profile_picture text,
    password        text        NOT NULL,
    license_plates  text[]      NOT NULL DEFAULT '{}'
);

-- UNIQUE, which is what makes `find_by_email`'s LIMIT 1 a fact about the schema
-- rather than a hope about the data.
CREATE UNIQUE INDEX app_user_email_idx ON app_user (email);

-- ─── refresh_token ──────────────────────────────────────────────────────────
-- `user_id` was `record<user>` in SurrealDB, which meant the column was written as
-- `user:⟨uuid⟩` and read back unwrapped — two halves in two different statements
-- with nothing in Rust connecting them, and a whole live test existing to prove they
-- agreed. It is a plain uuid with a foreign key now, so there is nothing to agree.
CREATE TABLE refresh_token (
    id             uuid PRIMARY KEY,
    version        bigint      NOT NULL DEFAULT 0,
    user_id        uuid        NOT NULL REFERENCES app_user (id) ON DELETE CASCADE,
    token_hash     text        NOT NULL,
    jti            uuid        NOT NULL,
    created_at     timestamptz NOT NULL,
    expires_at     timestamptz NOT NULL,
    revoked        boolean     NOT NULL DEFAULT false,
    revoked_reason text
);

-- UNIQUE and load-bearing: every lookup of a refresh token goes through this column
-- and expects exactly one row. A token hash is SHA-256 of a random v4 uuid, so the
-- constraint rejects nothing that could legitimately occur.
CREATE UNIQUE INDEX refresh_token_hash_idx ON refresh_token (token_hash);

-- The sweeper's delete: everything already past its expiry.
CREATE INDEX refresh_token_expires_idx ON refresh_token (expires_at);

-- ─── leader election ────────────────────────────────────────────────────────
-- One row, `leader`, and it exists for the OUTBOX RELAY only.
--
-- The projectors do not need it: they pull from one durable consumer per partition
-- with max_ack_pending 1, so JetStream hands out one event at a time across every
-- replica, in order. The relay has no such backstop — two relays could publish one
-- aggregate's events out of order, which the downstream `WHERE status IN …` guards
-- drop rather than reorder.
--
-- Expiry is compared against now() INSIDE the database. Replicas do not share a
-- clock; they do share this row.
CREATE TABLE _lease (
    name       text PRIMARY KEY,
    holder     text        NOT NULL,
    expires_at timestamptz NOT NULL
);

-- ─── transactional outbox ───────────────────────────────────────────────────
-- Events written in the SAME transaction as the data that caused them, then moved to
-- NATS by the relay. This is the one thing JetStream cannot provide on its own: its
-- guarantees begin once a message is in the stream, and this closes the window
-- before that.
--
-- The primary key is the envelope's event_id, so enqueuing the same event twice is
-- one row — the same property Nats-Msg-Id gives on the way out.
CREATE TABLE _outbox (
    id         uuid PRIMARY KEY,
    subject    text        NOT NULL,
    -- The serialized envelope, as text so a stuck row can be read with a SELECT.
    -- That is the entire reason this table is worth looking at when something has
    -- gone wrong, and jsonb would not improve it: nothing ever queries into it.
    payload    text        NOT NULL,
    -- Relay order, not business time. Written by the database's clock rather than the
    -- enqueuing replica's — two replicas do not share a clock, and a replica running
    -- 50ms fast could otherwise stamp an aggregate's v3 before another stamped its v2.
    created_at timestamptz NOT NULL DEFAULT now()
);

-- The relay's read: oldest first, `created_at` then `id`. Both columns are in the
-- index this time — unlike SurrealDB, which rejected `id` here because it was the
-- record key rather than a field, leaving the tiebreak unindexed.
--
-- Range-sharded on purpose. Yugabyte hash-shards a primary key by default, which is
-- right for the uuidv7 keys everywhere else in this schema but wrong here: this table
-- is read exclusively in `created_at` order, and hash sharding would turn every drain
-- into a cross-tablet sort.
CREATE INDEX _outbox_order ON _outbox (created_at ASC, id ASC);
