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

DEFINE TABLE OVERWRITE user SCHEMAFULL
    PERMISSIONS FOR select WHERE id = $auth.id;

-- Assigned once at registration; selects the NATS subject this user's events
-- live on, so it must never change.
DEFINE FIELD OVERWRITE shard              ON user TYPE string;
DEFINE FIELD OVERWRITE first_name         ON user TYPE string;
DEFINE FIELD OVERWRITE last_name          ON user TYPE string;
DEFINE FIELD OVERWRITE email              ON user TYPE string ASSERT string::is_email($value);
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

DEFINE INDEX OVERWRITE refresh_token_hash ON refresh_token FIELDS token_hash;

-- Projector cursor for the USERS stream, written in the same transaction as the
-- rows it describes. See the note in spot-schema.surql.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
