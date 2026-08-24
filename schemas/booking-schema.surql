USE NS main DB main;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- ─── booking ────────────────────────────────────────────────────────────────
-- spot_id / owner_id are plain uuids: the spot table lives in the spot-service DB,
-- so this DB can't hold a record<spot> link. owner_id is denormalized in at create
-- time (frontend already has it from the spot) so a booking can be scoped to the
-- owner without a cross-DB dereference. renter_id comes from the JWT.
--
-- PERMISSIONS NONE, not a select clause. No browser reaches this database — it is
-- private to booking-service, which connects with a database-level user whose role
-- bypasses table permissions and authorizes in Rust from the verified JWT. The
-- `$token.ID` clause that used to sit here never constrained anything. Client reads
-- of bookings go through view-service; view-schema.surql holds the real clause.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS NONE;

DEFINE FIELD OVERWRITE spot_id    ON booking TYPE uuid;
DEFINE FIELD OVERWRITE owner_id   ON booking TYPE uuid;

-- Set explicitly by booking-service from the verified claim, carried in the event.
-- Never `VALUE $token.ID`: that forced the field from the *caller's* token, so this
-- row could only ever be written by a connection authenticated as that very user —
-- and silently coerced to NONE (rejecting the write) for the service identity that
-- actually writes it. Same bug the owner_id comment in spot-schema.surql describes.
DEFINE FIELD OVERWRITE renter_id  ON booking TYPE uuid;

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

-- When a hold lapses. **The only place a hold expiry is stored**, and deliberately
-- not part of what makes a booking block a slot: a 'reserved' row blocks, full stop.
-- A lapsed hold stops blocking when the sweeper publishes Released, not because
-- every reader learned to compare this against the clock.
DEFINE FIELD OVERWRITE hold_until     ON booking TYPE option<datetime>;
DEFINE FIELD OVERWRITE release_reason ON booking TYPE option<string>
    ASSERT $value = NONE OR $value IN ['abandoned', 'expired'];
DEFINE FIELD OVERWRITE cancel_reason  ON booking TYPE option<string>
    ASSERT $value = NONE OR $value IN ['by_renter', 'spot_unavailable'];

-- The last moment this booking occupies, as an instant. `booked` holds wall-clock
-- strings in the spot's zone, so "is it over yet" can't be asked of it in a query
-- without knowing that zone — the projector stores the folded answer instead.
DEFINE FIELD OVERWRITE ends_at ON booking TYPE datetime;

-- renter's score after the trip; feeds rating (rating_avg). NONE until they rate.
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>
    ASSERT $value = NONE OR $value IN [1, 2, 3, 4, 5];

-- No `VALUE time::now()` — see the note in spot-schema.surql. The projector sets
-- this from the event, because every replica must derive the same timestamp.
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

DEFINE INDEX OVERWRITE booking_renter ON booking FIELDS renter_id;
-- The expiry sweeper's query: status = 'reserved' AND hold_until < now.
DEFINE INDEX OVERWRITE booking_hold   ON booking FIELDS status, hold_until;
-- Both remaining per-spot queries: reserve's "what blocks these times" and the
-- cancel reactor's "what is still to come". Both are (spot_id =, ends_at >) with
-- status as a residual filter — `status` used to lead `ends_at` here, but reserve
-- asks `status NOT IN`, which cannot range-scan, so it belongs after the comparison
-- that can. A plain spot_id lookup is served by this index's prefix, which is why
-- there is no separate one.
DEFINE INDEX OVERWRITE booking_spot   ON booking FIELDS spot_id, ends_at;

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

DEFINE FIELD OVERWRITE owner_id       ON spot TYPE option<uuid>;
DEFINE FIELD OVERWRITE shard          ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE price_per_hour ON spot TYPE option<int>;      -- EUR cents
DEFINE FIELD OVERWRITE timezone       ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE active         ON spot TYPE bool DEFAULT true;
-- Withdrawn for good. Reserve already refuses `active = false`; this additionally
-- stops a hold taken *before* the deletion from being confirmed, because the host
-- has said they cannot provide the space at all.
DEFINE FIELD OVERWRITE deleted        ON spot TYPE bool DEFAULT false;

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

-- There is deliberately no `booked` map here. Which slots are taken is a query over
-- the booking rows above (spot_id + ends_at > now, minus released/cancelled), not a
-- denormalized copy that has to be kept in step with them.

-- The compare-and-swap cursor: the stream sequence of the last BOOKINGS event
-- applied for this spot, which is what reserve asserts as
-- `Nats-Expected-Last-Subject-Sequence`. Advanced in the same transaction as the
-- booking row that event wrote, so it can never run ahead of the rows reserve reads
-- — a cursor ahead of its data would authorise a booking against slots it hasn't
-- seen.
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
