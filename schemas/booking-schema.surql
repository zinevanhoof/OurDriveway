USE NS main DB main;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- ─── booking ────────────────────────────────────────────────────────────────
-- spot_id / owner_id are plain strings: the spot table lives in the spot-service DB,
-- so this DB can't hold a record<spot> link. owner_id is denormalized in at create
-- time (frontend already has it from the spot) so the select permission can scope to
-- the owner without a cross-DB dereference. renter_id comes from the JWT.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS
    FOR create WHERE true,
    FOR select, update WHERE renter_id = $token.ID OR owner_id = $token.ID;

DEFINE FIELD OVERWRITE spot_id    ON booking TYPE string;
DEFINE FIELD OVERWRITE owner_id   ON booking TYPE string;

-- No `VALUE $token.ID`. That forced the field from the *caller's* token, so this
-- row could only ever be written by a connection authenticated as that very user —
-- and silently coerced to NONE (rejecting the write) for the service identity that
-- actually writes it. Same bug the owner_id comment in spot-schema.surql describes.
-- booking-service sets it explicitly from the verified claim, carried in the event.
DEFINE FIELD OVERWRITE renter_id  ON booking TYPE string;

-- The spot's shard, denormalized so the expiry sweeper can rebuild this booking's
-- NATS subject without a second lookup.
DEFINE FIELD OVERWRITE spot_shard ON booking TYPE string;

-- A booking spans one or more dates, each with one or more time slots — same
-- single-like shape as spot availability (minus weekly). Key = "YYYY-MM-DD",
-- value = [{ start, end }] as "HH:MM" strings. Assumed in the spot's timezone,
-- so bookings store no timezone-aware datetimes.
DEFINE FIELD OVERWRITE booked           ON booking TYPE object;
DEFINE FIELD OVERWRITE booked.*         ON booking TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON booking TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON booking TYPE string;

-- EUR cents. Was `float`; binary floating point can't represent most decimal
-- prices exactly, and this is what the renter is actually charged. Matches
-- spot.price_per_hour and view-schema's booking.amount.
DEFINE FIELD OVERWRITE amount     ON booking TYPE int;

-- 'reserved' is a hold taken while checkout runs: it blocks the slots exactly like
-- a confirmed booking, and ends as 'confirmed' (paid), or 'released' (the renter
-- backed out, or the hold lapsed and the expiry sweeper collected it).
DEFINE FIELD OVERWRITE status     ON booking TYPE string DEFAULT 'reserved'
    ASSERT $value IN ['reserved', 'confirmed', 'released', 'completed', 'cancelled'];

-- When a hold lapses. **The only place a hold expiry is stored** — it is
-- deliberately not carried into spot.booked, so that map means "taken" with no
-- qualification and no reader has to filter it.
DEFINE FIELD OVERWRITE hold_until     ON booking TYPE option<datetime>;
DEFINE FIELD OVERWRITE release_reason ON booking TYPE option<string>
    ASSERT $value = NONE OR $value IN ['abandoned', 'expired'];

-- renter's score after the trip; feeds rating (rating_avg). NONE until they rate.
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>
    ASSERT $value = NONE OR $value IN [1, 2, 3, 4, 5];

-- No `VALUE time::now()` — see the note in spot-schema.surql. The projector sets
-- this from the event, because every replica must derive the same timestamp.
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

DEFINE INDEX OVERWRITE booking_spot   ON booking FIELDS spot_id;
DEFINE INDEX OVERWRITE booking_renter ON booking FIELDS renter_id;
-- The expiry sweeper's query: status = 'reserved' AND hold_until < now.
DEFINE INDEX OVERWRITE booking_hold   ON booking FIELDS status, hold_until;

-- ─── spot (local projection of the SPOTS stream) ────────────────────────────
-- booking-service needs price, availability and timezone to authorize and price a
-- booking server-side, and the spot table lives in another service's database.
--
-- The SPOTS-owned fields are `option<>` on purpose. The two projectors advance
-- independently, so on a cold rebuild a BOOKINGS event can arrive for a spot whose
-- SpotCreated hasn't been applied yet and create this row first. SCHEMAFULL would
-- reject a partial row otherwise. Reserve rejects a spot whose availability is
-- still absent, so the gap fails closed rather than booking against nothing.
DEFINE TABLE OVERWRITE spot SCHEMAFULL PERMISSIONS NONE;

DEFINE FIELD OVERWRITE owner_id       ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE shard          ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE price_per_hour ON spot TYPE option<int>;      -- EUR cents
DEFINE FIELD OVERWRITE timezone       ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE active         ON spot TYPE bool DEFAULT true;

DEFINE FIELD OVERWRITE availability ON spot TYPE option<object>;
DEFINE FIELD OVERWRITE availability.weekly ON spot TYPE object;
DEFINE FIELD OVERWRITE availability.weekly.monday    ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.tuesday   ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.wednesday ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.thursday  ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.friday    ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.saturday  ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.sunday    ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.weekly.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE availability.weekly.*.*.end   ON spot TYPE string;
DEFINE FIELD OVERWRITE availability.single ON spot TYPE object;
DEFINE FIELD OVERWRITE availability.single.*         ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE availability.single.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE availability.single.*.*.end   ON spot TYPE string;

-- Slots taken on this spot: "YYYY-MM-DD" -> [{ start, end }]. A full recompute of
-- the spot's booking rows on every BOOKINGS event, which is what makes applying an
-- event idempotent — appending would double up on a redelivery.
--
-- No hold_until and no booking_id here: everything in this map is taken, and a
-- lapsed hold is removed by the sweeper publishing Released rather than by readers
-- learning to skip it.
DEFINE FIELD OVERWRITE booked           ON spot TYPE object DEFAULT {};
DEFINE FIELD OVERWRITE booked.*         ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON spot TYPE string;

-- The compare-and-swap cursor: the stream sequence of the last BOOKINGS event
-- applied for this spot, which is what reserve asserts as
-- `Nats-Expected-Last-Subject-Sequence`. It lives on the same record as the slots
-- it summarises, written in the same statement, so the two can never disagree —
-- a cursor ahead of its data would authorise a booking against slots it hasn't seen.
DEFINE FIELD OVERWRITE bookings_seq ON spot TYPE int DEFAULT 0;

-- ─── projector cursors ──────────────────────────────────────────────────────
-- Where this instance's projectors have got to. Written in the same transaction as
-- the data they describe, so a crash can never leave a cursor ahead of the rows
-- (which would silently skip events on restart).
--
-- PERMISSIONS NONE keeps it invisible to any client identity; only the service's
-- database-level connection, whose role bypasses table permissions, touches it.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
