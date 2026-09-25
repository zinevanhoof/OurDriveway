// @generated automatically by Diesel CLI.

diesel::table! {
    _outbox (id) {
        id -> Uuid,
        subject -> Text,
        payload -> Text,
        created_at -> Timestamptz,
    }
}
