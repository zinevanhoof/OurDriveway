-- One database per service inside the shared `main` namespace. Was DB main for
-- every service, which was safe only while each ran its own SurrealDB over its
-- own datastore; on TiKV they share one keyspace and this name is the isolation.
USE NS main DB user;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- No `DEFINE USER` here any more. user-service signs in with the root credentials
-- the container is started with (SURREALDB_USER / SURREALDB_PASS, see
-- shared/src/db.rs), so there is no second credential to keep in sync — and no
-- password literal committed to the repo.

-- PERMISSIONS NONE, matching every other table in this file and the other private
-- schemas. As the header says, no record access is even defined here, so `$auth` is
-- never populated and the `id = $auth.id` clause that used to sit here could not
-- have constrained anything. The real per-user clause lives in view-schema.surql.
DEFINE TABLE OVERWRITE user SCHEMAFULL PERMISSIONS NONE;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`user:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON user TYPE int DEFAULT 0;

DEFINE FIELD OVERWRITE first_name         ON user TYPE string;
DEFINE FIELD OVERWRITE last_name          ON user TYPE string;
DEFINE FIELD OVERWRITE email              ON user TYPE string ASSERT string::is_email($value);
-- Proven reachable: somebody opened a link only this mailbox received. Login
-- refuses while this is false, so it is an authentication field and lives only
-- here — view-service's `user` table is world-readable and must not carry it.
-- Reset to false whenever the address changes, or the flag would still be
-- describing the previous one.
DEFINE FIELD OVERWRITE email_verified     ON user TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE profile_picture    ON user TYPE option<string>;
DEFINE FIELD OVERWRITE password           ON user TYPE string PERMISSIONS NONE;

DEFINE FIELD OVERWRITE license_plates     ON user TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE license_plates.*   ON user TYPE string;

DEFINE INDEX OVERWRITE email_idx          ON user FIELDS email UNIQUE;

DEFINE TABLE OVERWRITE refresh_token SCHEMAFULL;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`refresh_token:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON refresh_token TYPE int DEFAULT 0;

DEFINE FIELD OVERWRITE user_id            ON refresh_token TYPE record<user>;
DEFINE FIELD OVERWRITE token_hash         ON refresh_token TYPE string;
DEFINE FIELD OVERWRITE jti                ON refresh_token TYPE uuid;
DEFINE FIELD OVERWRITE created_at         ON refresh_token TYPE datetime;
DEFINE FIELD OVERWRITE expires_at         ON refresh_token TYPE datetime;
DEFINE FIELD OVERWRITE revoked            ON refresh_token TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE revoked_reason     ON refresh_token TYPE option<string>;

-- UNIQUE, which it was not until now. Every lookup of a refresh token goes
-- through this column and expects exactly one row — `Repository::find_by` only
-- accepts a column marked `#[row(unique)]`, and that marker is a claim about
-- *this* line. A token hash is SHA-256 of a random v4 uuid, so the constraint
-- rejects nothing that could legitimately occur.
DEFINE INDEX OVERWRITE refresh_token_hash ON refresh_token FIELDS token_hash UNIQUE;


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
