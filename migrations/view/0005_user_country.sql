-- Where a user banks, mirrored from the USERS stream.
--
-- Scoped like `email` rather than like `license_plates`: it is cut per-caller in
-- `projections::user::OwnerViewUser`, so only the profile screen ever reads it back.
-- The plates are on the public side because a host has to recognise the car on their
-- driveway; nobody has that kind of reason to know where somebody banks.
--
-- Nullable forever. It is asked for on the profile screen and only hosts need it — see
-- `migrations/user/0002_user_country.sql`, which is the source of truth for this value.
ALTER TABLE app_user ADD COLUMN country text;
