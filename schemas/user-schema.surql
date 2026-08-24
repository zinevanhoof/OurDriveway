USE NS main DB main;

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

-- Assigned once at registration; selects the NATS subject this user's events
-- live on, so it must never change.
DEFINE FIELD OVERWRITE shard              ON user TYPE string;
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

DEFINE FIELD OVERWRITE user_id            ON refresh_token TYPE record<user>;
-- The user's shard, carried on the event so sessions publish to the same subject
-- as the rest of that user's history.
DEFINE FIELD OVERWRITE shard              ON refresh_token TYPE string;
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

-- Projector cursor for the USERS stream, written in the same transaction as the
-- rows it describes. See the note in spot-schema.surql.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
