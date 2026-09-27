-- When this account last had a verification or password-reset email queued.
--
-- The per-account cooldown on the two unauthenticated "email me" endpoints reads and
-- writes it — see user-service's `policy::mail`. One column for both, so the limit is
-- on mail sent to the address, whichever form asked for it.
--
-- Nullable: NULL is "never", which is every existing account.
ALTER TABLE app_user ADD COLUMN mail_requested_at timestamptz;
