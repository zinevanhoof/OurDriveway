-- What `GET /api/view/renter/bookings/next` seeks on.
--
-- `booking_renter` is `(renter_id)` alone, which answers "which are mine" and then
-- sorts whatever comes back. The next-up card asks a narrower question —
-- `renter_id = $1 AND ends_at > now() ORDER BY ends_at LIMIT 1` — and with `ends_at` in
-- the index that is a one-row seek instead of a scan of a renter's whole history.
--
-- The existing list read (`ORDER BY ends_at DESC`) is served by the same index in the
-- other direction, so `booking_renter` has nothing left that this does not do better.
-- It is dropped rather than kept: two indexes over the same leading column are two
-- writes per booking to answer one question.
CREATE INDEX booking_renter_ends ON booking (renter_id, ends_at);
DROP INDEX booking_renter;
