//! Small adapters between what diesel generates and what the domain models want.

use diesel::sql_types::{Nullable, Numeric};

diesel::define_sql_function! {
    /// `numeric` -> `bigint`, which `sum()` needs and `.cast()` cannot do.
    ///
    /// Postgres widens `SUM(bigint)` to `numeric` to avoid overflow, and `numeric` does
    /// not decode into an `i64` without pulling in bigdecimal. Diesel 2.3 has a native
    /// `.cast::<ST>()`, but its allowlist is a fixed set of pairs
    /// (`diesel::expression::cast`) and `numeric` appears in none of them.
    ///
    /// Postgres exposes every cast as an ordinary function too — `int8(numeric)` is what
    /// `::bigint` compiles to — so this reaches it with no migration and no new
    /// dependency. It rounds, which is a no-op at every call site: the summands are
    /// always a whole number of cents.
    ///
    /// **Nullable in and nullable out on purpose.** `sum()` over no rows is NULL, and a
    /// host who has never been paid is the ordinary case — so it arrives as `None` and
    /// the caller writes `.unwrap_or(0)`. That is deliberately not `COALESCE(…, 0)` in
    /// SQL: coalescing inside the database flattens "no rows" and "sums to zero" into one
    /// value before Rust can tell them apart.
    ///
    /// Used by all three services that total money. See
    /// `WalletRepository::balance_for_host` for the shape.
    #[sql_name = "int8"]
    fn to_bigint(x: Nullable<Numeric>) -> Nullable<BigInt>
}

// `Version(u64)` was here: a newtype with `Queryable`, `FromSql`, `ToSql` and both
// `From`s, so that a `version: u64` field could live in a `bigint` column. Every model
// carries `version: i64` now, which is the column's own type, so diesel needs nothing —
// and the three `sql_query` reads that clamped by hand (`shared::db` twice,
// `bus::await_version` once) needed nothing either, because `QueryableByName` goes to
// `FromSql` and never saw `deserialize_as`. What the unsigned type was defending against
// is a negative in the column, which only a hand-written UPDATE can produce; `next_version`
// clamps that one read, and `parse_version` still refuses a negative from the one input a
// client controls.

/// Teach a serde type to be a `jsonb` column, in place.
///
/// The replacement for `#[sqlx(json)]`, which was one field attribute. diesel needs the
/// *type* to speak the wire format, so this generates the `FromSql`/`ToSql` pair; the
/// type additionally derives `AsExpression` and `FromSqlRow` where it is defined.
///
/// ## Why not `diesel_json::Json<T>`
///
/// `diesel_json` is the obvious answer and was the plan of record: it decodes through
/// `serde_json::Value` and propagates the real serde error, which is what the other
/// jsonb crate gets wrong. Its cost is that `Json<T>` is visible in the *field type* —
/// `Spot.address` becomes `Json<Address>` — and `Deref` covers reads but not
/// construction. That is **129 sites** across the domain models, the projectors and the
/// services, none of which is about JSON.
///
/// `deserialize_as`/`serialize_as` would keep the field type, but the conversion needs
/// `impl From<Json<Address>> for Address`, and that runs into the orphan rule from the
/// wrong side.
///
/// So the impls go directly on the types, which is allowed because `Address`,
/// `Availability` and `Booked` are all local to this crate. Same error quality — the
/// serde error is propagated, not swallowed — and no wrapper anywhere.
#[macro_export]
macro_rules! jsonb_column {
    ($t:ty) => {
        impl ::diesel::deserialize::FromSql<::diesel::sql_types::Jsonb, ::diesel::pg::Pg> for $t {
            fn from_sql(bytes: ::diesel::pg::PgValue<'_>) -> ::diesel::deserialize::Result<Self> {
                let value = <::serde_json::Value as ::diesel::deserialize::FromSql<
                    ::diesel::sql_types::Jsonb,
                    ::diesel::pg::Pg,
                >>::from_sql(bytes)?;
                // `?` on the serde error, never a discarded one: a projector that stalls
                // has to be able to say WHICH field failed to decode.
                Ok(::serde_json::from_value(value)?)
            }
        }

        impl ::diesel::serialize::ToSql<::diesel::sql_types::Jsonb, ::diesel::pg::Pg> for $t {
            fn to_sql<'b>(
                &'b self,
                out: &mut ::diesel::serialize::Output<'b, '_, ::diesel::pg::Pg>,
            ) -> ::diesel::serialize::Result {
                let value = ::serde_json::to_value(self)?;
                // Through an owned `Value` written into a fresh buffer, because the
                // borrow `ToSql` hands out must outlive this call and a local cannot.
                <::serde_json::Value as ::diesel::serialize::ToSql<
                    ::diesel::sql_types::Jsonb,
                    ::diesel::pg::Pg,
                >>::to_sql(&value, &mut out.reborrow())
            }
        }
    };
}

// `BookedJson` was here, and `NullableBooked` below it: a wrapper applied with
// `deserialize_as` at ten field sites, plus a second one for the wallet's single
// `Option<Booked>`. Both existed because `Booked` was a **type alias** for a `HashMap`,
// so the orphan rule kept `jsonb_column!` from putting the impls on it — and kept
// `Option<BookedJson> -> Option<Booked>` from being written at all.
//
// `Booked` is a newtype now and carries the impls itself, exactly as `Address` and
// `Availability` do, so `Option<Booked>` decodes through diesel's own blanket nullable
// impl. `SpotMirror::availability` was always the proof this worked.

// `TextArray` was here: a `Vec<Option<String>>` -> `Vec<String>` narrowing applied to
// `images` and `license_plates` with `deserialize_as`, because `print-schema` types every
// Postgres array as `Array<Nullable<Text>>`. The generated modules now say `Array<Text>`
// (`scripts/schema-patches/`), so `Vec<String>` is diesel's own decode and needs nothing
// here. The behaviour that changed with it: a NULL element is now a decode error rather
// than being dropped — see the note in `schema/mod.rs`.

// `NullableBooked` also carried a `FromSql` impl with a `from_nullable_sql` override, on
// the grounds that the wallet read through `sql_query` and `QueryableByName` routes to
// `FromSql` rather than `Queryable`. That query has been fully DSL since the note on
// `WalletRepository::balance_for_host` was written, so the impl was already dead before
// the newtype removed the need for the wrapper at all.
