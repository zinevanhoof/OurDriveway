USE NS main DB main;

DEFINE ACCESS account
ON DATABASE
TYPE RECORD
WITH JWT
ALGORITHM HS256
KEY "super-secret";

DEFINE CONFIG GRAPHQL AUTO;

DEFINE TABLE post SCHEMAFULL
PERMISSIONS
    FOR create WHERE true,
    FOR select WHERE author = $token.ID,
    FOR update WHERE author = $token.ID,
    FOR delete WHERE author = $token.ID;

DEFINE FIELD title ON post
    TYPE string;

DEFINE FIELD content ON post
    TYPE string;

DEFINE FIELD author ON post
    TYPE string
    VALUE $token.ID
    READONLY;