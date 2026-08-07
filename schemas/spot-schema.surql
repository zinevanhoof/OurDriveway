USE NS main DB main;

-- This database is PRIVATE to its service. No GraphQL config and no record
-- access are defined, so a client JWT cannot authenticate against it at all —
-- client reads are served by view-service from the combined projection.
-- Only the service's own database-level connection touches these tables.

-- Every DEFINE is OVERWRITE: the datastore is now persistent (surrealkv volume),
-- so --import-file re-runs this file against an existing database on every boot.
-- Without OVERWRITE the second boot fails on redefinition.

-- Distance (meters) from a spot's point to a lng/lat pair. Exists so the map's
-- radius query can filter through auto GraphQL's `call` operator: geo::distance
-- needs a geometry, which can't be passed as a JSON filter arg, so we take plain
-- floats and rebuild the point here.
DEFINE FUNCTION OVERWRITE fn::spot_distance($loc: geometry, $lng: float, $lat: float) {
    RETURN geo::distance($loc, type::point([$lng, $lat]));
};

-- Reads are open (active spots are public listings; an owner also sees their own
-- inactive ones). Writes are impossible for any client identity: spot-service
-- connects with a database-level user, whose role bypasses table permissions, and
-- authorizes ownership in Rust from the verified JWT.
--
-- This was `PERMISSIONS FULL`, which let any authenticated user UPDATE or DELETE
-- anyone else's spot through the proxied GraphQL/SQL endpoint.
DEFINE TABLE OVERWRITE spot SCHEMAFULL PERMISSIONS
    FOR select WHERE active = true OR owner_id = $token.ID,
    FOR create, update, delete NONE;

-- No `VALUE $token.ID`: that forced the field from the caller's token, so the
-- table could only be written by a connection authenticated as that very user —
-- and silently coerced to NONE (rejecting the write) for any service identity.
-- spot-service now sets owner_id explicitly from the verified claim.
DEFINE FIELD OVERWRITE owner_id ON spot TYPE string;
-- Assigned once at creation and never changed: it selects the NATS subject this
-- spot's events live on, and moving a spot between shards would split its history.
DEFINE FIELD OVERWRITE shard ON spot TYPE string;
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

-- Where this instance's projector has got to in the SPOTS stream. Written in the
-- same transaction as the data it describes, so a crash can never leave the
-- cursor ahead of the rows (which would silently skip events on restart).
--
-- PERMISSIONS NONE keeps it invisible to any client identity; only the service's
-- database-level connection, whose role bypasses table permissions, touches it.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
