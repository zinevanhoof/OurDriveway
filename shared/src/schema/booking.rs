// @generated automatically by Diesel CLI.

diesel::table! {
    booking (id) {
        id -> Uuid,
        version -> Int8,
        spot_id -> Uuid,
        host_id -> Uuid,
        renter_id -> Uuid,
        booked -> Jsonb,
        amount -> Int8,
        status -> Text,
        hold_until -> Nullable<Timestamptz>,
        release_reason -> Nullable<Text>,
        cancel_reason -> Nullable<Text>,
        ends_at -> Timestamptz,
        rating -> Nullable<Int4>,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    spot (id) {
        id -> Uuid,
        version -> Int8,
        host_id -> Nullable<Uuid>,
        price_per_hour -> Nullable<Int8>,
        timezone -> Nullable<Text>,
        availability -> Nullable<Jsonb>,
        active -> Bool,
        deleted -> Bool,
    }
}

diesel::allow_tables_to_appear_in_same_query!(booking, spot,);
