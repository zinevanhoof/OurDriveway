// @generated automatically by Diesel CLI.

diesel::table! {
    _lease (name) {
        name -> Text,
        holder -> Text,
        expires_at -> Timestamptz,
    }
}

diesel::table! {
    _outbox (id) {
        id -> Uuid,
        subject -> Text,
        payload -> Text,
        created_at -> Timestamptz,
    }
}

diesel::allow_tables_to_appear_in_same_query!(_lease, _outbox,);
