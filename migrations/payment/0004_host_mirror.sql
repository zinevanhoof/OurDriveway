-- A mirror of the USERS stream, for the two fields Stripe demands before it will open
-- a connected account.
--
-- ─── why a mirror and not a lookup ──────────────────────────────────────────
--
-- The obvious alternative is a request/reply to user-service, which is what
-- `shared::rpc` exists for — and what its own module docs forbid here:
--
--   Everything here is best-effort by construction. […] nothing load-bearing may
--   travel this way […] If the answer not arriving would break something, it belongs
--   in an event.
--
-- Onboarding cannot happen without these two values, so user-service being down would
-- take payouts down with it. They travel as events instead, exactly like the `booking`
-- mirror one table over.
--
-- ─── two columns, and no more ───────────────────────────────────────────────
--
-- Not a copy of the user. A name, a password hash, a profile picture and a list of
-- licence plates have no business in the most private database in the system; these two
-- are here because `POST /v2/core/accounts` refuses without them.
CREATE TABLE host (
    id      uuid PRIMARY KEY,
    version bigint NOT NULL DEFAULT 0,

    -- `contact_email` on the connected account. Stripe: "If configuration.recipient is
    -- supplied, the Account must have a contact email."
    email   text   NOT NULL,

    -- `identity.country`, ISO 3166-1 alpha-2. NULL until the host fills it in on their
    -- profile, which is why `ConnectService::account_session` has a branch for its
    -- absence rather than a default: Stripe fixes this value permanently at account
    -- creation, so guessing it wrong costs a host their payouts for good.
    country text
);

-- No index beyond the key. Every read is `WHERE id = $1` with the owner from a verified
-- claim; there is no query across this table and never will be.
