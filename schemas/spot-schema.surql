-- One database per service inside the shared `main` namespace. Was DB main for
-- every service, which was safe only while each ran its own SurrealDB over its
-- own datastore; on TiKV they share one keyspace and this name is the isolation.
USE NS main DB spot;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- PERMISSIONS NONE, not a select clause. No browser reaches this database — it is
-- private to spot-service, which connects with a database-level user whose role
-- bypasses table permissions entirely and authorizes ownership in Rust from the
-- verified JWT. The `$token.ID` clause that used to sit here therefore never
-- constrained anything; removing it takes away nothing but the impression that
-- this database is client-facing. Client reads of spots go through view-service,
-- and view-schema.surql is where a real permission clause lives.
DEFINE TABLE OVERWRITE spot SCHEMAFULL PERMISSIONS NONE;

-- Monotonic, bumped by the owning service inside the transaction that writes the
-- rows. Three jobs in one column: the token a client waits on
-- (`spot:<id>@7`), the key two concurrent writers collide on so TiKV refuses
-- one, and the gap detector that stops an out-of-order event applying
-- silently.
DEFINE FIELD OVERWRITE version ON spot TYPE int DEFAULT 0;

-- Set explicitly by spot-service from the verified claim. Never `VALUE $token.ID`:
-- that forced the field from the caller's token, so the table could only be written
-- by a connection authenticated as that very user — and silently coerced to NONE
-- (rejecting the write) for any service identity.
DEFINE FIELD OVERWRITE owner_id ON spot TYPE uuid;
DEFINE FIELD OVERWRITE title ON spot TYPE string;
DEFINE FIELD OVERWRITE description ON spot TYPE option<string>;
-- EUR cents. Was `float`; binary floating point can't represent most decimal
-- prices exactly, and this feeds what a renter is actually charged.
DEFINE FIELD OVERWRITE price_per_hour ON spot TYPE int;
DEFINE FIELD OVERWRITE images ON spot TYPE array<string>;
DEFINE FIELD OVERWRITE location ON spot TYPE geometry<point>;
DEFINE FIELD OVERWRITE active ON spot TYPE bool DEFAULT true;
-- Two different "off" states. `active = false` is the host's live switch: no new
-- reservations, bookings already taken are still honoured. `deleted` is permanent —
-- the row survives only so a renter's past bookings can still resolve a title and
-- an address, and every list filters it out.
DEFINE FIELD OVERWRITE deleted ON spot TYPE bool DEFAULT false;

DEFINE FIELD OVERWRITE address ON spot TYPE object;

DEFINE FIELD OVERWRITE address.line1 ON spot TYPE string;
DEFINE FIELD OVERWRITE address.line2 ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE address.city ON spot TYPE string;
DEFINE FIELD OVERWRITE address.postal_code ON spot TYPE string;
DEFINE FIELD OVERWRITE address.region ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE address.country ON spot TYPE string;
DEFINE FIELD OVERWRITE address.formatted ON spot TYPE string;

DEFINE FIELD OVERWRITE availability ON spot TYPE object;

DEFINE FIELD OVERWRITE availability.weekly ON spot TYPE object;

DEFINE FIELD OVERWRITE availability.weekly.monday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.tuesday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.wednesday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.thursday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.friday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.saturday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.sunday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE availability.weekly.*.*.end ON spot TYPE string;

DEFINE FIELD OVERWRITE availability.single ON spot TYPE object;

DEFINE FIELD OVERWRITE availability.single.* ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.single.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE availability.single.*.*.end ON spot TYPE string;

DEFINE FIELD OVERWRITE timezone ON spot TYPE string;

-- No `VALUE time::now()`. These are written by Rust, because once spots are
-- projected from an event stream every replica must derive the same timestamp
-- from the event — a clock read in the database would differ per instance.
-- (It was also a plain bug: `VALUE` re-evaluates on UPDATE, so created_at
-- tracked the last write rather than the first.)
DEFINE FIELD OVERWRITE created_at ON spot TYPE datetime;
DEFINE FIELD OVERWRITE updated_at ON spot TYPE datetime;

DEFINE INDEX OVERWRITE spot_owner ON spot FIELDS owner_id;


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
