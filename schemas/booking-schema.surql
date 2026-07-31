USE NS main DB main;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- spot_id / owner_id are plain strings: the spot table lives in the spot-service DB,
-- so this DB can't hold a record<spot> link. owner_id is denormalized in at create
-- time (frontend already has it from the spot) so the select permission can scope to
-- the owner without a cross-DB dereference. renter_id comes from the JWT.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS
    FOR create WHERE true,
    FOR select, update WHERE renter_id = $token.ID OR owner_id = $token.ID;

DEFINE FIELD OVERWRITE spot_id    ON booking TYPE string;
DEFINE FIELD OVERWRITE owner_id   ON booking TYPE string;
DEFINE FIELD OVERWRITE renter_id  ON booking TYPE string VALUE $token.ID;

-- A booking spans one or more dates, each with one or more time slots — same
-- single-like shape as spot availability (minus weekly). Key = "YYYY-MM-DD",
-- value = [{ start, end }] as "HH:MM" strings. Assumed in the spot's timezone,
-- so bookings store no timezone-aware datetimes.
DEFINE FIELD OVERWRITE booked           ON booking TYPE object;
DEFINE FIELD OVERWRITE booked.*         ON booking TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON booking TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON booking TYPE string;

DEFINE FIELD OVERWRITE amount     ON booking TYPE float;   -- what the renter paid, feeds earned (amount_sum)

DEFINE FIELD OVERWRITE status     ON booking TYPE string DEFAULT 'confirmed'
    ASSERT $value IN ['confirmed', 'completed', 'cancelled'];

-- renter's score after the trip; feeds rating (rating_avg). NONE until they rate.
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>
    ASSERT $value = NONE OR $value IN [1, 2, 3, 4, 5];

-- No `VALUE time::now()` — see the note in spot-schema.surql. Nothing writes
-- bookings today; the projector sets this from the event when booking lands.
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

DEFINE INDEX OVERWRITE booking_spot ON booking FIELDS spot_id;
