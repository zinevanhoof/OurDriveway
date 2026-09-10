// @generated automatically by Diesel CLI.

diesel::table! {
    app_user (id) {
        id -> Uuid,
        version -> Int8,
        first_name -> Text,
        last_name -> Text,
        email -> Text,
        email_verified -> Bool,
        profile_picture -> Nullable<Text>,
        password -> Text,
        license_plates -> Array<Text>,
        country -> Nullable<Text>,
    }
}

diesel::table! {
    refresh_token (id) {
        id -> Uuid,
        version -> Int8,
        user_id -> Uuid,
        token_hash -> Text,
        jti -> Uuid,
        created_at -> Timestamptz,
        expires_at -> Timestamptz,
        revoked -> Bool,
        revoked_reason -> Nullable<Text>,
    }
}

diesel::joinable!(refresh_token -> app_user (user_id));

diesel::allow_tables_to_appear_in_same_query!(app_user, refresh_token,);
