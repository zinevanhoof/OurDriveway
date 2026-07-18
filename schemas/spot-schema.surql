USE NS main DB main;

DEFINE ACCESS account
ON DATABASE
TYPE RECORD
WITH JWT
ALGORITHM HS256
KEY "super-secret";

DEFINE CONFIG GRAPHQL AUTO;

-- Distance (meters) from a spot's point to a lng/lat pair. Exists so the map's
-- radius query can filter through auto GraphQL's `call` operator: geo::distance
-- needs a geometry, which can't be passed as a JSON filter arg, so we take plain
-- floats and rebuild the point here.
DEFINE FUNCTION fn::spot_distance($loc: geometry, $lng: float, $lat: float) {
    RETURN geo::distance($loc, type::point([$lng, $lat]));
};

DEFINE TABLE spot SCHEMAFULL PERMISSIONS FULL;

DEFINE FIELD owner_id ON spot TYPE string VALUE $token.ID;

DEFINE FIELD title ON spot TYPE string;
DEFINE FIELD description ON spot TYPE option<string>;

DEFINE FIELD price_per_hour ON spot TYPE float;

DEFINE FIELD images ON spot TYPE array<string>;

DEFINE FIELD location ON spot TYPE geometry<point>;

DEFINE FIELD address ON spot TYPE object;

DEFINE FIELD address.line1 ON spot TYPE string;
DEFINE FIELD address.line2 ON spot TYPE option<string>;

DEFINE FIELD address.city ON spot TYPE string;
DEFINE FIELD address.postal_code ON spot TYPE string;
DEFINE FIELD address.region ON spot TYPE option<string>;
DEFINE FIELD address.country ON spot TYPE string;

DEFINE FIELD address.formatted ON spot TYPE string;



DEFINE FIELD availability ON spot TYPE object;



DEFINE FIELD availability.weekly ON spot TYPE object;

DEFINE FIELD availability.weekly.monday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.tuesday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.wednesday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.thursday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.friday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.saturday ON spot TYPE array<object>;
DEFINE FIELD availability.weekly.sunday ON spot TYPE array<object>;



DEFINE FIELD availability.weekly.*.*.start ON spot TYPE string;
DEFINE FIELD availability.weekly.*.*.end ON spot TYPE string;



DEFINE FIELD availability.single ON spot TYPE object;

DEFINE FIELD availability.single.* ON spot TYPE array<object>;

DEFINE FIELD availability.single.*.*.start ON spot TYPE string;
DEFINE FIELD availability.single.*.*.end ON spot TYPE string;


DEFINE FIELD timezone ON spot TYPE string;


DEFINE FIELD created_at ON spot TYPE datetime VALUE time::now();
DEFINE FIELD updated_at ON spot TYPE datetime VALUE time::now();