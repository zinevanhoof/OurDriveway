USE NS main DB main;

-- ─────────────────────────────────────────────────────────────────────────────
-- The combined read model. Projected from every service's event stream, with
-- real record links, and exposed to browsers through SurrealDB's auto GraphQL.
--
-- TWO IDENTITIES, ONE DATABASE — this is the whole security model:
--
--   writes  view-service's own connection, signed in as a database USER whose
--           ROLE bypasses table permissions. Never reachable from a browser.
--   reads   the GraphQL endpoint, reverse-proxied with the client's own
--           `Authorization: Bearer` forwarded verbatim. That grants a RECORD
--           identity, which has no role and therefore cannot bypass anything.
--
-- Every table below declares `FOR create, update, delete NONE`, so a write on
-- the read path is structurally impossible rather than merely unrouted. There
-- are no GraphQL mutations to reach, and if a future SurrealDB adds them, they
-- still hit this wall.
--
-- Nothing here is a source of truth. Delete the volume and it rebuilds from the
-- log. Never make a decision (availability, pricing, authorization) from data in
-- this database — those reads belong to the service that owns them.
-- ─────────────────────────────────────────────────────────────────────────────

DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD
WITH JWT ALGORITHM HS256 KEY "super-secret";

DEFINE CONFIG OVERWRITE GRAPHQL AUTO;

-- Distance (meters) from a spot's point to a lng/lat pair. Must exist here too:
-- the map's radius query reaches it through auto GraphQL's `call` filter
-- operator, and that is the single most fragile query in the app.
DEFINE FUNCTION OVERWRITE fn::spot_distance($loc: geometry, $lng: float, $lat: float) {
    RETURN geo::distance($loc, type::point([$lng, $lat]));
};

-- ─── user ───────────────────────────────────────────────────────────────────
-- Public-safe by construction: no password, and NO EMAIL is projected here at
-- all. `WHERE true` is required because spot-owner profiles have to resolve for
-- everyone — which is also why "who am I" can't be a query against this table
-- and is served by GET /api/view/me instead.
--
-- If a sensitive field ever lands here, scope it at FIELD level, e.g.
--   DEFINE FIELD email ON user PERMISSIONS FOR select WHERE id = $token.ID
-- Field permissions blank a value; they do not filter rows.
DEFINE TABLE OVERWRITE user SCHEMAFULL PERMISSIONS
    FOR select WHERE true,
    FOR create, update, delete NONE;

DEFINE FIELD OVERWRITE first_name      ON user TYPE string;
DEFINE FIELD OVERWRITE last_name       ON user TYPE string;
DEFINE FIELD OVERWRITE profile_picture ON user TYPE option<string>;

-- ─── spot ───────────────────────────────────────────────────────────────────
DEFINE TABLE OVERWRITE spot SCHEMAFULL PERMISSIONS
    FOR select WHERE active = true OR owner_id = $token.ID,
    FOR create, update, delete NONE;

-- `owner` is the link that makes nested GraphQL work; `owner_id` is the plain
-- string everything actually *depends* on. Permissions and GraphQL filters use
-- the string, never the link: streams have no cross-stream ordering, so a spot
-- can arrive before the user it references and the link is NONE until backfilled.
-- A permission traversing `owner.…` would silently deny access during that gap.
DEFINE FIELD OVERWRITE owner          ON spot TYPE option<record<user>>;
DEFINE FIELD OVERWRITE owner_id       ON spot TYPE string;
DEFINE FIELD OVERWRITE title          ON spot TYPE string;
DEFINE FIELD OVERWRITE description    ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE price_per_hour ON spot TYPE int;          -- EUR cents
DEFINE FIELD OVERWRITE images         ON spot TYPE array<string>;
DEFINE FIELD OVERWRITE location       ON spot TYPE geometry<point>;
DEFINE FIELD OVERWRITE active         ON spot TYPE bool DEFAULT true;

DEFINE FIELD OVERWRITE address ON spot TYPE object;
DEFINE FIELD OVERWRITE address.line1       ON spot TYPE string;
DEFINE FIELD OVERWRITE address.line2       ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE address.city        ON spot TYPE string;
DEFINE FIELD OVERWRITE address.postal_code ON spot TYPE string;
DEFINE FIELD OVERWRITE address.region      ON spot TYPE option<string>;
DEFINE FIELD OVERWRITE address.country     ON spot TYPE string;
DEFINE FIELD OVERWRITE address.formatted   ON spot TYPE string;

DEFINE FIELD OVERWRITE availability ON spot TYPE object;
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

DEFINE FIELD OVERWRITE timezone   ON spot TYPE string;
DEFINE FIELD OVERWRITE created_at ON spot TYPE datetime;
DEFINE FIELD OVERWRITE updated_at ON spot TYPE datetime;

DEFINE INDEX OVERWRITE spot_owner  ON spot FIELDS owner_id;
DEFINE INDEX OVERWRITE spot_active ON spot FIELDS active;

-- ─── booking (defined, not yet projected) ───────────────────────────────────
-- Declared now so the frontend can move to this endpoint in one go. Nothing
-- publishes booking events yet, so these return empty — exactly what the old
-- per-service query returned, so no behaviour regresses.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS
    FOR select WHERE renter_id = $token.ID OR owner_id = $token.ID,
    FOR create, update, delete NONE;

DEFINE FIELD OVERWRITE spot      ON booking TYPE option<record<spot>>;
DEFINE FIELD OVERWRITE renter    ON booking TYPE option<record<user>>;
DEFINE FIELD OVERWRITE spot_id   ON booking TYPE string;
DEFINE FIELD OVERWRITE owner_id  ON booking TYPE string;
DEFINE FIELD OVERWRITE renter_id ON booking TYPE string;
DEFINE FIELD OVERWRITE booked           ON booking TYPE object;
DEFINE FIELD OVERWRITE booked.*         ON booking TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON booking TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON booking TYPE string;
DEFINE FIELD OVERWRITE amount     ON booking TYPE int;           -- EUR cents
DEFINE FIELD OVERWRITE status     ON booking TYPE string;
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>;
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

DEFINE INDEX OVERWRITE booking_spot   ON booking FIELDS spot_id;
DEFINE INDEX OVERWRITE booking_renter ON booking FIELDS renter_id;

-- ─── spot_busy ──────────────────────────────────────────────────────────────
-- Occupied slots per (spot, date), with NO renter identity attached, so it is
-- safe to read publicly.
--
-- This replaces querying `booking` for availability. That query was a live bug:
-- `booking` is only selectable by its renter or the spot owner, so a prospective
-- renter got an empty list back and every slot looked free.
DEFINE TABLE OVERWRITE spot_busy SCHEMAFULL PERMISSIONS
    FOR select WHERE true,
    FOR create, update, delete NONE;

DEFINE FIELD OVERWRITE spot_id ON spot_busy TYPE string;
DEFINE FIELD OVERWRITE date    ON spot_busy TYPE string;         -- "YYYY-MM-DD"
DEFINE FIELD OVERWRITE slots   ON spot_busy TYPE array<object>;
DEFINE FIELD OVERWRITE slots.*.start ON spot_busy TYPE string;
DEFINE FIELD OVERWRITE slots.*.end   ON spot_busy TYPE string;

DEFINE INDEX OVERWRITE spot_busy_spot ON spot_busy FIELDS spot_id;

-- ─── projector cursors ──────────────────────────────────────────────────────
-- One row per stream (_projection:USERS, _projection:SPOTS, …), each written in
-- the same transaction as the data it describes.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
