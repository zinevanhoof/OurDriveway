use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::domain_models::view::notification::{NotificationPayload, ViewNotification};
use shared::error::myerror::MyResult;
use shared::schema::view::{app_user, booking, notification, spot};
use uuid::Uuid;

/// The `notification` table, plus the one `app_user` column it is read against.
pub struct NotificationRepository;

impl NotificationRepository {
    /// Insert, or leave the existing row alone. A replayed event must not resurrect a
    /// notification that was handled since.
    pub async fn insert(conn: &mut AsyncPgConnection, row: ViewNotification) -> MyResult<()> {
        diesel::insert_into(notification::table)
            .values(row)
            .on_conflict_do_nothing()
            .execute(conn)
            .await?;
        Ok(())
    }

    /// What a confirmed booking announces: "someone booked your spot" to the host, now,
    /// and "rate this booking" to the renter, dated at its end.
    ///
    /// Read off the booking row rather than the event, which carries only an id. A row
    /// that is not confirmed — the `Confirmed` guard matched nothing — gets neither.
    pub async fn insert_for_confirmed(
        conn: &mut AsyncPgConnection,
        booking_id: Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        let found: Option<(Uuid, Uuid, Uuid, DateTime<Utc>, Option<String>, Option<String>)> =
            booking::table
                .left_join(spot::table.on(spot::id.eq(booking::spot_id)))
                .left_join(app_user::table.on(app_user::id.eq(booking::renter_id)))
                .filter(booking::id.eq(booking_id).and(booking::status.eq("confirmed")))
                .select((
                    booking::renter_id,
                    booking::host_id,
                    booking::spot_id,
                    booking::ends_at,
                    spot::title.nullable(),
                    app_user::first_name.nullable(),
                ))
                .first(conn)
                .await
                .optional()?;

        let Some((renter_id, host_id, spot_id, ends_at, spot_title, renter_name)) = found else {
            return Ok(());
        };

        let booked = NotificationPayload::SpotBooked {
            booking_id,
            spot_id,
            spot_title: spot_title.clone(),
            renter_name,
        };
        Self::insert(conn, ViewNotification::new(host_id, booked, at)).await?;

        let rate = NotificationPayload::RateBooking {
            booking_id,
            spot_title,
        };
        Self::insert(conn, ViewNotification::new(renter_id, rate, ends_at)).await
    }

    /// Marks every open notification of these kinds about this subject as handled,
    /// whoever it belongs to — the event that finishes them is about the subject.
    pub async fn handle(
        conn: &mut AsyncPgConnection,
        subject_id: Uuid,
        kinds: &[&str],
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        diesel::update(
            notification::table.filter(
                notification::subject_id
                    .eq(subject_id)
                    .and(notification::kind.eq_any(kinds))
                    .and(notification::handled_at.is_null()),
            ),
        )
        .set(notification::handled_at.eq(Some(at)))
        .execute(conn)
        .await?;
        Ok(())
    }

    /// One user dismissed one of their own. `user_id` is in the `WHERE`, so a dismissal
    /// can only ever reach the dismisser's row.
    pub async fn dismiss(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
        kind: &str,
        subject_id: Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        diesel::update(
            notification::table.filter(
                notification::subject_id
                    .eq(subject_id)
                    .and(notification::kind.eq(kind))
                    .and(notification::user_id.eq(user_id))
                    .and(notification::handled_at.is_null()),
            ),
        )
        .set(notification::handled_at.eq(Some(at)))
        .execute(conn)
        .await?;
        Ok(())
    }

    /// What the caller has not handled and may already see, newest first.
    ///
    /// ponytail: no page — the list is bounded by what is still open. Page on
    /// `visible_from` if a kind ever piles up.
    pub async fn find_open_for_account(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
        now: DateTime<Utc>,
    ) -> MyResult<Vec<ViewNotification>> {
        Ok(notification::table
            .filter(
                notification::user_id
                    .eq(user_id)
                    .and(notification::handled_at.is_null())
                    .and(notification::visible_from.le(now)),
            )
            .order(notification::visible_from.desc())
            .select(ViewNotification::as_select())
            .load(conn)
            .await?)
    }

    /// When the caller last opened their notifications. `None` for never, and for a
    /// user whose row has not been projected yet.
    pub async fn seen_at(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
    ) -> MyResult<Option<DateTime<Utc>>> {
        Ok(app_user::table
            .find(user_id)
            .select(app_user::notifications_seen_at)
            .first(conn)
            .await
            .optional()?
            .flatten())
    }

    /// Moves the watermark forward, never back: an older event applied late must not
    /// make seen notifications new again.
    pub async fn mark_seen(
        conn: &mut AsyncPgConnection,
        user_id: Uuid,
        at: DateTime<Utc>,
    ) -> MyResult<()> {
        diesel::update(
            app_user::table.find(user_id).filter(
                app_user::notifications_seen_at
                    .is_null()
                    .or(app_user::notifications_seen_at.lt(at)),
            ),
        )
        .set(app_user::notifications_seen_at.eq(Some(at)))
        .execute(conn)
        .await?;
        Ok(())
    }
}
