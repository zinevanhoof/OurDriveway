-- Where a user banks — ISO 3166-1 alpha-2, uppercase.
--
-- Nullable, and it stays nullable: it is asked for on the profile screen and only
-- hosts ever need it. Stripe will not open a connected account without a country and
-- fixes it permanently at creation (Accounts v2 `identity.country`), which is the whole
-- reason it is collected explicitly rather than guessed from an address or a locale.
--
-- No CHECK on the format. `shared::requests::user::Country` owns that rule — two ASCII
-- letters, uppercased on the way in — and it is the only way a value reaches here.
ALTER TABLE app_user ADD COLUMN country text;
