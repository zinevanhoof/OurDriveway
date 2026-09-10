//! One diesel schema module per service database. **Generated — do not hand-edit.**
//!
//! ```sh
//! docker compose -f docker/docker-compose-dev.yml up -d yugabyte
//! scripts/migrate.sh
//! scripts/print-schema.sh
//! ```
//!
//! Read back out of a live, migrated cluster rather than parsed from `migrations/`, so
//! regeneration needs the dev stack up and is a manual step after every migration. CI has
//! no database and cannot check these are current; a stale one surfaces as a compile
//! error at the query naming the column that moved, not here.
//!
//! ## Five modules, deliberately not one
//!
//! `user::app_user` and `view::app_user` are different tables with different columns —
//! the read model's copy has no password and no `email_verified`. One global schema
//! module would make those collide, and the type-level separation is what stops a query
//! written against one database from compiling against another.
//!
//! `_lease` and `_outbox` are **absent** from all five, though every database has them —
//! each service runs its own outbox relay and its own leader election. They are filtered
//! out here (`diesel.toml`) and generated into `bus::schema` instead, by their own pass in
//! `scripts/print-schema.sh`. Every table is still generated exactly once; the pass that
//! writes it just belongs to the crate that owns it.
//!
//! That is not tidiness. `bus::outbox::enqueue` is generic over whichever database its
//! caller holds, so it cannot name a per-service type — and because these modules do not
//! declare them, `bus::schema::_outbox` is the only `_outbox` that exists. Services reach
//! the outbox through `bus` because nothing else compiles.
//!
//! ## `Array<Text>` — patched, not generated
//!
//! `images` and `license_plates` are `text[] NOT NULL`, but diesel generates
//! `Array<Nullable<Text>>` for every Postgres array: the column being NOT NULL says
//! nothing about its *elements*, and Postgres will happily store `{a,NULL,b}`. So the
//! generated type is honest and the fields would have to be `Vec<Option<String>>`.
//!
//! Nothing in this codebase ever writes a NULL element, so `scripts/schema-patches/`
//! narrows those three columns to `Array<Text>` and the fields are a plain
//! `Vec<String>`. The patches are applied by `scripts/print-schema.sh` itself — that is
//! what stops a regeneration from silently reverting the edit — and they fail the run
//! loudly if the surrounding columns move.
//!
//! The consequence to know: a NULL element is now a **decode error**, not a dropped
//! element. Diesel's array decoder calls `String::from_nullable_sql(None)`, which is an
//! "Unexpected null". Only a hand-written `UPDATE` in psql can produce one.

pub mod booking;
pub mod payment;
pub mod spot;
pub mod user;
pub mod view;
