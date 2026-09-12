use axum_extra::extract::cookie::{Cookie, SameSite};
use cookie::{CookieBuilder, time::Duration};

use crate::CONFIG;

const NAME: &str = "refresh-token";

/// The path the refresh-token cookie is scoped to, and the reason the session
/// endpoints share a prefix.
///
/// A browser sends a cookie only to this path and its `/`-delimited descendants
/// (RFC 6265 §5.1.4), so scoping it here keeps a 30-day credential off signup,
/// verification, profile edits and password changes — every request that has no
/// use for it. Widening this to `/api/user` would put it on all of them; that is
/// the trade this prefix exists to avoid.
///
/// Login is deliberately *not* under it: login mints the cookie rather than
/// receiving one, and `Set-Cookie` is unaffected by the request path.
pub const SESSION_PATH: &str = "/api/user/session";

/// Issues the cookie. One builder for both sides, because a `remove` only clears
/// a cookie whose name, path and domain match the one that set it — three
/// hand-copied attribute lists is a silent "logout didn't log out" waiting to
/// happen.
pub fn set(value: String) -> Cookie<'static> {
    build(Cookie::build((NAME, value)))
        // Was a hardcoded 30 days while the token itself expires after
        // REFRESH_TOKEN_EXPIRATION (15). The browser kept sending a dead token for
        // the difference, turning a clean logout into a fortnight of 401s.
        .max_age(Duration::days(CONFIG.refresh_token_expiration))
        .build()
}

/// Clears it. Valid to send from any path — deleting a cookie does not require
/// the browser to have sent it.
pub fn clear() -> Cookie<'static> {
    build(Cookie::build(NAME)).build()
}

/// The attributes both sides must agree on.
fn build(builder: CookieBuilder<'static>) -> CookieBuilder<'static> {
    builder
        .http_only(true)
        .same_site(SameSite::Strict)
        .path(SESSION_PATH)
}
