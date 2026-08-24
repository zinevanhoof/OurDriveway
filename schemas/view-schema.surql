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
-- FIELD level: every row stays selectable, but the email reads as null for
-- anyone who isn't its owner. Field permissions cut a key from the projected
-- output; they do not filter rows and they do not touch what is stored — which
-- is why the type has to be `option<string>` even though nothing ever writes
-- NONE. A non-`option` kind renders as `T!` in auto GraphQL, and a cut key on a
-- non-null field nulls the whole parent object. Anything else sensitive added
-- later belongs behind the same clause, with the same `option<>`.
--
-- `$auth.id` compares record id to record id. Every key in this database is a
-- uuid and the JWT claim is `user:u'<uuid>'`, so `$auth` resolves to this very
-- row — which also means this clause hits SurrealDB's self-reference guard and
-- costs no extra read. The sibling tables below compare `record::id($auth)`
-- against a plain `uuid` field instead; do NOT reach for `$auth.<field>` there,
-- as that dereference fetches the whole user record per row.
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
    FOR select WHERE active = true OR owner_id = record::id($auth),
    FOR create, update, delete NONE;

-- `owner` is the link that makes nested GraphQL work; `owner_id` is the plain
-- uuid everything actually *depends* on. Permissions and GraphQL filters use the
-- uuid, never the link: streams have no cross-stream ordering, so a spot can
-- arrive before the user it references and the link is NONE until backfilled.
-- A permission traversing `owner.…` would silently deny access during that gap.
DEFINE FIELD OVERWRITE owner          ON spot TYPE option<record<user>>;
DEFINE FIELD OVERWRITE owner_id       ON spot TYPE uuid;
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

-- There is deliberately no `booked` map here. Which slots are taken is a query over
-- the `booking` table below, whose select permission admits the rows that block a
-- slot to everyone — see the note on that table.

DEFINE FIELD OVERWRITE timezone   ON spot TYPE string;
DEFINE FIELD OVERWRITE created_at ON spot TYPE datetime;
DEFINE FIELD OVERWRITE updated_at ON spot TYPE datetime;

DEFINE INDEX OVERWRITE spot_owner  ON spot FIELDS owner_id;
DEFINE INDEX OVERWRITE spot_active ON spot FIELDS active;

-- ─── booking ────────────────────────────────────────────────────────────────
-- The renter's and owner's view of a booking, and the source of availability for
-- everyone else.
--
-- The third select clause is what replaced `spot.booked`: a stranger sees exactly
-- the rows that block a slot, and nothing else. It is a ROW filter, which is why no
-- client has to remember to exclude released and cancelled bookings — they are
-- simply not selectable unless you are the renter or the owner.
--
-- Trade-off, stated so it is not discovered later: booking ids, their slots, their
-- ends_at and reserved-vs-confirmed become enumerable per spot. Renter identity,
-- amounts and hold expiry do not — those are scoped at FIELD level below.
--
-- With one measured exception. A WHERE is normally evaluated against the *reduced*
-- document, so a field the caller cannot read compares as NONE and cannot be probed
-- (surrealdb-core 3.2.4, doc/check.rs::check_where_condition). An INDEXED field
-- skips that path: an equality filter is answered from the index, whose entries are
-- raw. So `where: { renter_id: { eq: … } }` answers truthfully for a caller who
-- cannot read `renter_id`, purely because `booking_renter` exists — whether a given
-- user holds a current booking on a given spot is a boolean anyone can ask, and
-- user ids are public. Verified both ways: adding an index to `amount` made it
-- probeable, removing it made it stop. See vulns.txt.
--
-- Only `eq` leaks (range filters fall back to the document path), and only for the
-- indexed columns — `renter_id` and `owner_id`. Dropping `booking_renter` would
-- close it and turn every renter's own booking list into a table scan.
DEFINE TABLE OVERWRITE booking SCHEMAFULL PERMISSIONS
    FOR select WHERE renter_id = record::id($auth)
                  OR owner_id  = record::id($auth)
                  OR status IN ['reserved', 'confirmed'],
    FOR create, update, delete NONE;

-- Public, because these four ARE the availability answer: which spot, which slots,
-- until when, and whether it still blocks.
DEFINE FIELD OVERWRITE spot      ON booking TYPE option<record<spot>>;
DEFINE FIELD OVERWRITE spot_id   ON booking TYPE uuid;
DEFINE FIELD OVERWRITE booked           ON booking TYPE object;
DEFINE FIELD OVERWRITE booked.*         ON booking TYPE array<object>;
DEFINE FIELD OVERWRITE booked.*.*.start ON booking TYPE string;
DEFINE FIELD OVERWRITE booked.*.*.end   ON booking TYPE string;
DEFINE FIELD OVERWRITE status     ON booking TYPE string
    ASSERT $value IN ['reserved', 'confirmed', 'released', 'completed', 'cancelled'];

-- Everything below is scoped to the two parties, the same way `user.email` is.
--
-- **The `option<>` is load-bearing and not about storage.** The projector always
-- writes a real value; a denied field is cut from the projected OUTPUT, and the
-- resolver then hands GraphQL a null. Auto GraphQL renders any non-`option` kind as
-- `T!`, so that null violates the schema and propagates up through the non-null list
-- element — nulling the whole `bookings` array, which reads to a client as "nothing
-- is booked". Exactly the bug this table's select clause exists to prevent.
DEFINE FIELD OVERWRITE renter    ON booking TYPE option<record<user>>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
DEFINE FIELD OVERWRITE owner_id  ON booking TYPE option<uuid>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
DEFINE FIELD OVERWRITE renter_id ON booking TYPE option<uuid>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
DEFINE FIELD OVERWRITE amount    ON booking TYPE option<int>      -- EUR cents
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
-- Lets the renter's own checkout show a countdown. The public availability answer
-- carries no expiry at all — a lapsed hold stops blocking when the sweeper
-- publishes Released, not because a reader learned to skip it.
DEFINE FIELD OVERWRITE hold_until     ON booking TYPE option<datetime>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
DEFINE FIELD OVERWRITE release_reason ON booking TYPE option<string>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
-- 'spot_unavailable' is how the renter's row can say the *host* withdrew, rather
-- than showing the same bare "cancelled" they'd see for their own doing.
DEFINE FIELD OVERWRITE cancel_reason  ON booking TYPE option<string>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);
DEFINE FIELD OVERWRITE rating     ON booking TYPE option<int>
    PERMISSIONS FOR select WHERE renter_id = record::id($auth)
                              OR owner_id  = record::id($auth);

-- The last moment this booking occupies, folded from `booked` in the spot's zone at
-- reserve time. Upcoming-vs-past is a filter on this rather than a fold every client
-- repeats — and a fold is not something a GraphQL `where` can express at all.
DEFINE FIELD OVERWRITE ends_at    ON booking TYPE datetime;
DEFINE FIELD OVERWRITE created_at ON booking TYPE datetime;

-- The availability query and the owner's upcoming list are both
-- (spot_id =, ends_at >), so one composite serves both — and its prefix still
-- serves a plain spot_id lookup, which is why there is no separate index for that.
DEFINE INDEX OVERWRITE booking_spot   ON booking FIELDS spot_id, ends_at;
DEFINE INDEX OVERWRITE booking_renter ON booking FIELDS renter_id;

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
    FOR select WHERE owner_id = record::id($auth),
    FOR create, update, delete NONE;

-- `owner` is the link that makes nested GraphQL work; `owner_id` is the plain uuid
-- the permission clause above compares against. Same pairing as booking.renter.
DEFINE FIELD OVERWRITE owner      ON payout TYPE option<record<user>>;
DEFINE FIELD OVERWRITE owner_id   ON payout TYPE uuid;
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
