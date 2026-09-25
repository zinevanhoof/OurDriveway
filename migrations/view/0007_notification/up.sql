-- In-app notifications: one row per (thing, kind, person), written by the projector.
--
-- `visible_from` is what lets a notification be due in the future without a sweeper:
-- a rating prompt is inserted when the booking is confirmed, dated at its `ends_at`,
-- and the read simply does not return it before then.
--
-- The key leads with `subject_id` because that is what the projector knows when it
-- marks one handled — `Rated` carries a booking id, not a renter. The user's list is
-- served by the partial index below, which holds only what is still open.
CREATE TABLE notification (
  subject_id   uuid        NOT NULL,
  kind         text        NOT NULL,
  user_id      uuid        NOT NULL,
  data         jsonb       NOT NULL,
  visible_from timestamptz NOT NULL,
  handled_at   timestamptz,
  PRIMARY KEY (subject_id, kind, user_id)
);

CREATE INDEX notification_open ON notification (user_id, visible_from DESC)
  WHERE handled_at IS NULL;

-- When the user last opened their notifications. Anything visible before it reads as
-- seen; it drives the badge and nothing else.
ALTER TABLE app_user ADD COLUMN notifications_seen_at timestamptz;
