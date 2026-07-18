USE NS main DB main;

DEFINE USER leeroy
ON DATABASE
PASSWORD "SuperSecretPassword!"
ROLES OWNER;

DEFINE ACCESS account
ON DATABASE
TYPE RECORD
WITH JWT
ALGORITHM HS256
KEY "super-secret";

DEFINE CONFIG GRAPHQL TABLES INCLUDE user FUNCTIONS NONE;

DEFINE TABLE user SCHEMAFULL
    PERMISSIONS FOR select WHERE id = $auth.id;

DEFINE FIELD email ON user TYPE string ASSERT string::is_email($value);
DEFINE FIELD password ON user TYPE string PERMISSIONS NONE;

DEFINE INDEX email_idx ON user FIELDS email UNIQUE;



DEFINE TABLE refresh_token SCHEMAFULL;

DEFINE FIELD user_id ON refresh_token
    TYPE record<user>;

DEFINE FIELD token_hash ON refresh_token
    TYPE string;

DEFINE FIELD jti ON refresh_token
    TYPE uuid;

DEFINE FIELD created_at ON refresh_token
    TYPE datetime;

DEFINE FIELD expires_at ON refresh_token
    TYPE datetime;

DEFINE FIELD revoked ON refresh_token
    TYPE bool
    DEFAULT false;

DEFINE FIELD revoked_reason ON refresh_token
    TYPE option<string>;

DEFINE INDEX refresh_token_hash
    ON refresh_token
    FIELDS token_hash;