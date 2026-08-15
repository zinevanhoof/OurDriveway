use shared::{
    error::myerror::MyResult,
    events::{
        Envelope,
        booking::{BookingEvent, BookingReserved},
        payment::PaymentEvent,
        spot::{SpotCreated, SpotEvent, SpotUpdated},
        user::{UserEvent, UserRegistered, UserUpdated},
    },
    general_models::booking::{BookingRow, fold_booked},
};
use surrealdb::{Surreal, engine::remote::ws::Client, types::Datetime};
use uuid::Uuid;

/// Writes the combined read model.
///
/// Uses the service's own database-level connection, whose role bypasses the
/// `FOR create, update, delete NONE` on every table — that clause exists to stop
/// the *browser*, which reaches the same database through the GraphQL proxy with
/// only a record identity.
pub struct ViewRepository {
    pub db: Surreal<Client>,
}

/// Why a booking left its previous status, and so which field records it.
///
/// One value rather than two `Option`s because they are mutually exclusive by
/// construction: a hold that lapsed has no cancel reason, and a paid booking that
/// was withdrawn has no release reason. `release_reason` and `cancel_reason` stay
/// separate *fields* so a renter's history can tell "your hold ran out" from "the
/// host pulled the listing" — the status alone no longer distinguishes them.
enum Reason<'a> {
    None,
    Release(&'a str),
    Cancel(&'a str),
}

impl<'a> Reason<'a> {
    fn fields(self) -> (Option<&'a str>, Option<&'a str>) {
        match self {
            Self::None => (None, None),
            Self::Release(r) => (Some(r), None),
            Self::Cancel(r) => (None, Some(r)),
        }
    }
}

impl ViewRepository {
    pub async fn last_seq(&self, stream: &str) -> MyResult<u64> {
        let seq: Option<i64> = self
            .db
            .query("SELECT VALUE last_seq FROM ONLY type::record('_projection', $s)")
            .bind(("s", stream.to_string()))
            .await?
            .take(0)?;
        Ok(seq.unwrap_or(0).max(0) as u64)
    }

    // ─── users ──────────────────────────────────────────────────────────────

    pub async fn apply_user(&self, envelope: Envelope<UserEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            UserEvent::Registered(e) => self.user_registered(e, at, seq).await,
            UserEvent::Updated(e) => self.user_updated(e, at, seq).await,
            // Nothing to project — the hash never comes near this database. The
            // cursor still has to move, or a restart replays from before it.
            UserEvent::PasswordChanged(_) => self.bump_cursor("USERS", at, seq).await,
            // Verification state is an authentication concern and stays in
            // user-service's private projection. This table is world-readable, so
            // adding `email_verified` here would publish which addresses are
            // unconfirmed to every client that can read a spot owner's profile.
            UserEvent::EmailVerified { .. } | UserEvent::VerificationRequested(_) => {
                self.bump_cursor("USERS", at, seq).await
            }
        }
    }

    /// Advances a projector cursor for an event that changes no rows here.
    async fn bump_cursor(
        &self,
        stream: &str,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query("UPSERT type::record('_projection', $s) SET last_seq = $seq, updated_at = $at")
            .bind(("s", stream.to_string()))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn user_registered(
        &self,
        e: UserRegistered,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // Note what is absent: no password hash, ever. This table is
        // world-readable, so the projection is the first thing deciding what can
        // possibly leak. `email` is the one exception and it is projected behind
        // a field-level `WHERE id = $auth.id` in view-schema.surql — a row stays
        // selectable by anyone, the address does not.
        //
        // The two UPDATEs are the backfill. Streams have no cross-stream
        // ordering, so a spot may already be sitting here with `owner = NONE`
        // pointing at a user that hadn't arrived yet. Idempotent, so replaying
        // or racing another consumer is harmless.
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('user', $id) CONTENT {
                     first_name: $first_name, last_name: $last_name,
                     profile_picture: NONE, email: $email, license_plates: []
                 };
                 UPDATE spot SET owner = type::record('user', $id)
                     WHERE owner_id = $id AND owner = NONE;
                 UPDATE booking SET renter = type::record('user', $id)
                     WHERE renter_id = $id AND renter = NONE;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.user_id))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("email", e.email))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn user_updated(
        &self,
        e: UserUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('user', $id) SET
                     first_name      = $first_name      ?? first_name,
                     last_name       = $last_name       ?? last_name,
                     profile_picture = $profile_picture ?? profile_picture,
                     email           = $email           ?? email,
                     license_plates  = $license_plates  ?? license_plates;
                 UPSERT _projection:USERS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.user_id))
            .bind(("first_name", e.first_name))
            .bind(("last_name", e.last_name))
            .bind(("profile_picture", e.profile_picture))
            .bind(("email", e.email))
            .bind(("license_plates", e.license_plates))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    // ─── spots ──────────────────────────────────────────────────────────────

    pub async fn apply_spot(&self, envelope: Envelope<SpotEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            SpotEvent::Created(e) => self.spot_created(e, at, seq).await,
            SpotEvent::Updated(e) => self.spot_updated(e, at, seq).await,
            SpotEvent::Deactivated { spot_id } => {
                self.spot_set_active(spot_id, false, at, seq).await
            }
            SpotEvent::Activated { spot_id } => self.spot_set_active(spot_id, true, at, seq).await,
            SpotEvent::Deleted { spot_id } => self.spot_deleted(spot_id, at, seq).await,
        }
    }

    async fn spot_created(
        &self,
        e: SpotCreated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // `owner` resolves to NONE when the owner's UserRegistered hasn't been
        // applied yet; the user projector backfills it on arrival. `owner_id` is
        // always written, and that is what permissions and filters use.
        //
        // MERGE, not CONTENT. CONTENT replaces the whole record body with exactly
        // the listed keys, which silently drops `booked`. The projectors advance
        // independently, so on any cold rebuild or snapshot restore this event can
        // land *after* the BOOKINGS projector has already written that spot's
        // slots — and BOOKINGS never replays them. Safe because SpotCreated always
        // carries every field, so there's no "absent means clear" case to get wrong.
        //
        // The `booked` line is the mirror-image fix, for bookings that were applied
        // before this row existed at all: the booking projector's UPDATE matched
        // nothing and was dropped. Recomputing here covers that ordering, and is
        // idempotent — same backfill idiom as `user_registered` above.
        let rows = self.bookings_for_spot(&e.spot_id).await?;
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('spot', $id) MERGE {
                     owner: (SELECT VALUE id FROM ONLY user
                             WHERE record::id(id) = $owner_id LIMIT 1),
                     owner_id: $owner_id, title: $title, description: $description,
                     price_per_hour: $price, images: $images,
                     location: type::point([$lng, $lat]), active: true,
                     address: $address, availability: $availability, booked: $booked,
                     timezone: $timezone, created_at: $at, updated_at: $at
                 };
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("booked", fold_booked(&rows, at)))
            .bind(("id", e.spot_id))
            .bind(("owner_id", e.owner_id))
            .bind(("title", e.title))
            .bind(("description", e.description))
            .bind(("price", e.price_per_hour_cents))
            .bind(("images", e.images))
            .bind(("lng", e.lng))
            .bind(("lat", e.lat))
            .bind(("address", e.address))
            .bind(("availability", e.availability))
            .bind(("timezone", e.timezone))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_updated(
        &self,
        e: SpotUpdated,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET
                     title          = $title        ?? title,
                     description    = $description  ?? description,
                     price_per_hour = $price        ?? price_per_hour,
                     images         = $images       ?? images,
                     availability   = $availability ?? availability,
                     updated_at     = $at;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", e.spot_id))
            .bind(("title", e.title))
            .bind(("description", e.description))
            .bind(("price", e.price_per_hour_cents))
            .bind(("images", e.images))
            .bind(("availability", e.availability))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn spot_set_active(
        &self,
        spot_id: Uuid,
        active: bool,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET active = $active, updated_at = $at;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", spot_id))
            .bind(("active", active))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    /// Soft delete. The row stays selectable so `booking.spot` still resolves for a
    /// renter's past bookings — the lists filter `deleted`, the permission doesn't.
    async fn spot_deleted(
        &self,
        spot_id: Uuid,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET
                     active = false, deleted = true, updated_at = $at;
                 UPSERT _projection:SPOTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", spot_id))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    // ─── bookings ───────────────────────────────────────────────────────────

    pub async fn apply_booking(&self, envelope: Envelope<BookingEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            BookingEvent::Reserved(e) => self.booking_reserved(e, at, seq).await,
            BookingEvent::Confirmed { booking_id } => {
                self.booking_settled(booking_id, "reserved", "confirmed", Reason::None, at, seq)
                    .await
            }
            BookingEvent::Released { booking_id, reason } => {
                self.booking_settled(
                    booking_id,
                    "reserved",
                    "released",
                    Reason::Release(reason.as_str()),
                    at,
                    seq,
                )
                .await
            }
            BookingEvent::Cancelled { booking_id, reason } => {
                self.booking_settled(
                    booking_id,
                    "confirmed",
                    "cancelled",
                    Reason::Cancel(reason.as_str()),
                    at,
                    seq,
                )
                .await
            }
        }
    }

    /// PAYMENTS, of which only payouts land in the read model.
    ///
    /// Every other variant advances the cursor and stores nothing. That is not a gap:
    /// what a renter was charged is payment-service's to answer, and duplicating it
    /// here would create a second version of the same money.
    pub async fn apply_payment(&self, envelope: Envelope<PaymentEvent>, seq: u64) -> MyResult<()> {
        let at = envelope.occurred_at;
        match envelope.payload {
            PaymentEvent::PayoutRequested {
                payout_id,
                owner_id,
                amount_cents,
                requested_at,
            } => {
                self.payout(payout_id, owner_id, amount_cents, requested_at, at, seq)
                    .await
            }

            // Cursor only. Storing nothing is right, but skipping the *cursor* would
            // replay every payment event forever.
            _ => {
                self.db
                    .query("UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;")
                    .bind(("at", Datetime::from(at)))
                    .bind(("seq", seq as i64))
                    .await?
                    .check()?;
                Ok(())
            }
        }
    }

    async fn payout(
        &self,
        payout_id: Uuid,
        owner_id: Uuid,
        amount_cents: i64,
        requested_at: chrono::DateTime<chrono::Utc>,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        self.db
            .query(
                "BEGIN;
                 UPSERT type::record('payout', $id) CONTENT {
                     owner: (SELECT VALUE id FROM ONLY user WHERE record::id(id) = $owner_id LIMIT 1),
                     owner_id: $owner_id, amount: $amount, created_at: $created_at
                 };
                 UPSERT _projection:PAYMENTS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", payout_id))
            .bind(("owner_id", owner_id))
            .bind(("amount", amount_cents))
            .bind(("created_at", Datetime::from(requested_at)))
            .bind(("seq", seq as i64))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        Ok(())
    }

    async fn booking_reserved(
        &self,
        e: BookingReserved,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        // `spot` and `renter` are the record links that make nested GraphQL work;
        // both resolve to NONE if the referenced event hasn't been projected yet,
        // and the string ids are what permissions actually use.
        self.db
            .query(
                "UPSERT type::record('booking', $id) CONTENT {
                     spot:   (SELECT VALUE id FROM ONLY spot WHERE record::id(id) = $spot_id LIMIT 1),
                     renter: (SELECT VALUE id FROM ONLY user WHERE record::id(id) = $renter_id LIMIT 1),
                     spot_id: $spot_id, owner_id: $owner_id, renter_id: $renter_id,
                     booked: $booked, amount: $amount, status: 'reserved',
                     hold_until: $expires_at, ends_at: $ends_at,
                     release_reason: NONE, cancel_reason: NONE, created_at: $at
                 };",
            )
            .bind(("id", e.booking_id))
            .bind(("spot_id", e.spot_id))
            .bind(("owner_id", e.owner_id))
            .bind(("renter_id", e.renter_id))
            .bind(("booked", e.booked))
            .bind(("amount", e.amount_cents))
            .bind(("expires_at", Datetime::from(e.expires_at)))
            .bind(("ends_at", Datetime::from(e.ends_at)))
            .bind(("at", Datetime::from(at)))
            .await?
            .check()?;
        self.refold_spot(&e.spot_id, at, seq).await
    }

    async fn booking_settled(
        &self,
        booking_id: Uuid,
        from: &str,
        status: &str,
        reason: Reason<'_>,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        let (release_reason, cancel_reason) = reason.fields();
        // Scoped to the status the event is allowed to leave. A payment landing
        // microseconds before the hold lapses, with the sweeper's expiry arriving
        // second, must not undo the confirmation — this WHERE clause is the whole
        // guard. It is a parameter because a cancel leaves 'confirmed', not
        // 'reserved', and a hardcoded 'reserved' would drop cancels *silently*:
        // the None branch below still advances the cursor.
        let spot_id: Option<Uuid> = self
            .db
            .query(
                "UPDATE type::record('booking', $id) SET
                     status = $status, hold_until = NONE,
                     release_reason = $release ?? release_reason,
                     cancel_reason  = $cancel  ?? cancel_reason
                 WHERE status = $from
                 RETURN VALUE spot_id;",
            )
            .bind(("id", booking_id))
            .bind(("from", from.to_string()))
            .bind(("status", status.to_string()))
            .bind(("release", release_reason.map(str::to_string)))
            .bind(("cancel", cancel_reason.map(str::to_string)))
            .await?
            .take(0)?;

        match spot_id {
            Some(id) => self.refold_spot(&id, at, seq).await,
            // The transition didn't apply. `booked` is unchanged, but the cursor
            // still has to advance or this event replays forever.
            None => {
                self.db
                    .query("UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;")
                    .bind(("at", Datetime::from(at)))
                    .bind(("seq", seq as i64))
                    .await?
                    .check()?;
                Ok(())
            }
        }
    }

    /// Recomputes a spot's `booked` map from its booking rows, and advances the
    /// cursor in the same transaction.
    ///
    /// `UPDATE`, not `UPSERT`: this must not create a spot row. The view's `spot`
    /// table is SCHEMAFULL with required fields a bookings-first partial row can't
    /// satisfy. Matching zero rows is fine — `spot_created` recomputes `booked`
    /// when the spot finally lands.
    async fn refold_spot(
        &self,
        spot_id: &Uuid,
        at: chrono::DateTime<chrono::Utc>,
        seq: u64,
    ) -> MyResult<()> {
        let rows = self.bookings_for_spot(spot_id).await?;
        self.db
            .query(
                "BEGIN;
                 UPDATE type::record('spot', $id) SET booked = $booked;
                 UPSERT _projection:BOOKINGS SET last_seq = $seq, updated_at = $at;
                 COMMIT;",
            )
            .bind(("id", *spot_id))
            .bind(("booked", fold_booked(&rows, at)))
            .bind(("at", Datetime::from(at)))
            .bind(("seq", seq as i64))
            .await?
            .check()?;
        Ok(())
    }

    async fn bookings_for_spot(&self, spot_id: &Uuid) -> MyResult<Vec<BookingRow>> {
        Ok(self
            .db
            .query("SELECT status, booked FROM booking WHERE spot_id = $spot_id")
            .bind(("spot_id", *spot_id))
            .await?
            .take(0)?)
    }

    // ─── reads ──────────────────────────────────────────────────────────────

    /// Profile for `GET /api/view/me`. `None` while the user's event is still in
    /// flight — the caller answers with the claim's id regardless, so this never
    /// has to 404.
    pub async fn profile(&self, user_id: &Uuid) -> MyResult<Option<crate::route::me::Profile>> {
        let profile: Option<crate::route::me::Profile> = self
            .db
            .query(
                "SELECT first_name, last_name, profile_picture
                 FROM ONLY type::record('user', $id)",
            )
            .bind(("id", *user_id))
            .await?
            .take(0)?;
        Ok(profile)
    }
}
