// @generated automatically by Diesel CLI.

diesel::table! {
    booking (id) {
        id -> Uuid,
        version -> Int8,
        spot_id -> Uuid,
        host_id -> Uuid,
        renter_id -> Uuid,
        amount_cents -> Int8,
        booked -> Jsonb,
        status -> Text,
        hold_until -> Nullable<Timestamptz>,
        ends_at -> Timestamptz,
        cancel_reason -> Nullable<Text>,
        release_reason -> Nullable<Text>,
    }
}

diesel::table! {
    connect_account (host_id) {
        host_id -> Uuid,
        stripe_account_id -> Text,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    host (id) {
        id -> Uuid,
        version -> Int8,
        email -> Text,
        country -> Nullable<Text>,
    }
}

diesel::table! {
    payment (id) {
        id -> Uuid,
        version -> Int8,
        booking_id -> Uuid,
        host_id -> Uuid,
        renter_id -> Uuid,
        amount_cents -> Int8,
        session_id -> Text,
        intent_id -> Nullable<Text>,
        status -> Text,
        refund_id -> Nullable<Text>,
        failure_reason -> Nullable<Text>,
        created_at -> Timestamptz,
        refunded_at -> Nullable<Timestamptz>,
    }
}

diesel::table! {
    payout (id) {
        id -> Uuid,
        version -> Int8,
        host_id -> Uuid,
        amount_cents -> Int8,
        created_at -> Timestamptz,
        status -> Text,
        transfer_id -> Nullable<Text>,
        failure_reason -> Nullable<Text>,
    }
}

diesel::allow_tables_to_appear_in_same_query!(booking, connect_account, host, payment, payout,);
