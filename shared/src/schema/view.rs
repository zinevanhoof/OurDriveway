// @generated automatically by Diesel CLI.

diesel::table! {
    app_user (id) {
        id -> Uuid,
        version -> Int8,
        first_name -> Text,
        last_name -> Text,
        profile_picture -> Nullable<Text>,
        email -> Text,
        license_plates -> Array<Text>,
        country -> Nullable<Text>,
    }
}

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
        rating -> Nullable<Int4>,
        ends_at -> Timestamptz,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    payment (id) {
        id -> Uuid,
        version -> Int8,
        booking_id -> Uuid,
        host_id -> Uuid,
        renter_id -> Uuid,
        amount -> Int8,
        status -> Text,
        created_at -> Timestamptz,
        refunded_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    payout (id) {
        id -> Uuid,
        version -> Int8,
        host_id -> Uuid,
        amount -> Int8,
        created_at -> Timestamptz,
        status -> Text,
    }
}

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

diesel::allow_tables_to_appear_in_same_query!(app_user, booking, payment, payout, spot,);
