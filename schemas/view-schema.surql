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
-- No password is projected here, ever. `WHERE true` on the table is required
-- because spot-owner profiles have to resolve for everyone — which is also why
-- "who am I" can't be a query against this table and is served by
-- GET /api/view/me instead.
--
-- `email` is the one sensitive field that does land here, so it is scoped at
-- FIELD level: every row stays selectable, but the email reads as NONE for
-- anyone who isn't its owner. Field permissions blank a value; they do not
-- filter rows — which is exactly why the type has to be `option<string>`.
-- Anything else sensitive added later belongs behind the same clause.
--
-- `$auth.id`, NOT `$token.ID`. Both hold this user, but `id` here is a record
-- and `$token.ID` is the string "user:abc" — comparing them is always false, so
-- that version silently blanks the field for its owner too. `spot` below can
-- write `owner_id = $token.ID` only because `owner_id` is a plain string field.
DEFINE TABLE OVERWRITE user SCHEMAFULL PERMISSIONS
    FOR select WHERE true,
    FOR create, update, delete NONE;

DEFINE FIELD OVERWRITE first_name      ON user TYPE string;
DEFINE FIELD OVERWRITE last_name       ON user TYPE string;
DEFINE FIELD OVERWRITE profile_picture ON user TYPE option<string>;
DEFINE FIELD OVERWRITE email           ON user TYPE option<string>
    PERMISSIONS FOR select WHERE id = $auth.id;

-- Deliberately public: a host has to be able to recognise the car that turns up
-- on their driveway, so this is not scoped the way `email` is.
DEFINE FIELD OVERWRITE license_plates   ON user TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE license_plates.* ON user TYPE string;

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
-- A deleted spot stays selectable on purpose: `booking.spot` still links to it, and
-- a renter's past booking would otherwise render with no title and no address. It is
-- the *lists* that filter it out — see SPOTS_OWNED and the radius query.
DEFINE FIELD OVERWRITE deleted        ON spot TYPE bool DEFAULT false;

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

-- Slots taken on this spot: "YYYY-MM-DD" -> [{ start, end }], the same shape as
-- `availability.single` above so the client subtracts one from the other with no
-- reshaping. Recomputed from this spot's booking rows on every BOOKINGS event.
--
-- Safe to expose on a world-readable table: it carries no renter identity, no
-- booking id, and no hold expiry. That replaced a client query against `booking`
-- filtered by spot_id, which was a live bug — `booking` is only selectable by its
-- renter or the spot owner, so a prospective renter got an empty list back and
-- every slot looked free.
DEFINE FIELD OVERWRITE booked           ON spot TYPE object DEFAULT {};
DEFINE FIELD OVERWRITE booked.*         ON spot TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON spot TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON spot TYPE string;

DEFINE FIELD OVERWRITE timezone   ON spot TYPE string;
DEFINE FIELD OVERWRITE created_at ON spot TYPE datetime;
DEFINE FIELD OVERWRITE updated_at ON spot TYPE datetime;

DEFINE INDEX OVERWRITE spot_owner  ON spot FIELDS owner_id;
DEFINE INDEX OVERWRITE spot_active ON spot FIELDS active;

-- ─── booking ────────────────────────────────────────────────────────────────
-- The renter's and owner's view of a booking. Availability does NOT come from
-- here — a prospective renter can't select these rows at all — it comes from
-- `spot.booked` above.
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
DEFINE FIELD OVERWRITE status     ON booking TYPE string
    ASSERT $value IN ['reserved', 'confirmed', 'released', 'completed', 'cancelled'];
-- Lets the renter's own checkout show a countdown. Only ever readable by the
-- renter or the owner; `spot.booked` carries no expiry at all.
DEFINE FIELD OVERWRITE hold_until     ON booking TYPE option<datetime>;
DEFINE FIELD OVERWRITE release_reason ON booking TYPE option<string>;
-- 'spot_unavailable' is how the renter's row can say the *host* withdrew, rather
-- than showing the same bare "cancelled" they'd see for their own doing.
DEFINE FIELD OVERWRITE cancel_reason  ON booking TYPE option<string>;
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>;

-- The last moment this booking occupies, folded from `booked` in the spot's zone at
-- reserve time. Upcoming-vs-past is a filter on this rather than a fold every client
-- repeats — and a fold is not something a GraphQL `where` can express at all.
DEFINE FIELD OVERWRITE ends_at    ON booking TYPE datetime;
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

DEFINE INDEX OVERWRITE booking_spot   ON booking FIELDS spot_id;
DEFINE INDEX OVERWRITE booking_renter ON booking FIELDS renter_id;
DEFINE INDEX OVERWRITE booking_ends   ON booking FIELDS ends_at;

-- The home screen's "earned this week": SUM(amount) WHERE owner_id = me AND
-- created_at >= monday. Neither field was indexed, so that query planned as a
-- TableScan. Composite rather than owner_id alone — same one line, same one index
-- to maintain, but it range-scans the week instead of every booking the host has
-- ever had, and owner_id leads, so it still serves a plain owner_id lookup.
DEFINE INDEX OVERWRITE booking_owner_created ON booking FIELDS owner_id, created_at;

-- ─── payout ─────────────────────────────────────────────────────────────────
-- A host's withdrawal history, and nothing else about money.
--
-- Payments themselves are deliberately absent from this read model. What a renter was
-- charged and what a host has available are payment-service's to answer, and a second
-- copy here would eventually disagree with the balance shown next to the withdraw
-- button. This table exists so the *list* of withdrawals can be queried alongside the
-- rest of a profile, like everything else on this screen.
--
-- Nothing real moves when one of these is created — see the note on `payout` in
-- payment-schema.surql.
DEFINE TABLE OVERWRITE payout SCHEMAFULL PERMISSIONS
    FOR select WHERE owner_id = $token.ID,
    FOR create, update, delete NONE;

-- `owner` is the link that makes nested GraphQL work; `owner_id` is the plain string
-- the permission clause above compares against. Same pairing as booking.renter.
DEFINE FIELD OVERWRITE owner      ON payout TYPE option<record<user>>;
DEFINE FIELD OVERWRITE owner_id   ON payout TYPE string;
DEFINE FIELD OVERWRITE amount     ON payout TYPE int;           -- EUR cents
DEFINE FIELD OVERWRITE created_at ON payout TYPE datetime;

-- The history list: a host's own withdrawals, newest first.
DEFINE INDEX OVERWRITE payout_owner ON payout FIELDS owner_id, created_at;

-- ─── projector cursors ──────────────────────────────────────────────────────
-- One row per stream (_projection:USERS, _projection:SPOTS, …), each written in
-- the same transaction as the data it describes.
DEFINE TABLE OVERWRITE _projection SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE last_seq   ON _projection TYPE int;
DEFINE FIELD OVERWRITE updated_at ON _projection TYPE datetime;
