// @generated automatically by Diesel CLI.

diesel::table! {
    spot (id) {
        id -> Uuid,
        version -> Int8,
        host_id -> Uuid,
        title -> Text,
        description -> Nullable<Text>,
        price_per_hour -> Int8,
        images -> Array<Text>,
        lng -> Float8,
        lat -> Float8,
        active -> Bool,
        deleted -> Bool,
        address -> Jsonb,
        availability -> Jsonb,
        timezone -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}
