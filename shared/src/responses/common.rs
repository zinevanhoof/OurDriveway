//! What more than one handler puts in a response.

use axum::http::HeaderName;

/// Where a write's event landed, on the way back out.
///
/// The mirror of [`bus::AWAIT_VERSION`], which is the same version on the way back
/// **in**. It lives here rather than beside that one, which would put both halves of
/// the mechanism in a single file: `bus` depends on this crate and not the other way
/// round, and every writing handler needs the name.
///
/// A write answers `202` with this header and, usually, no body. 202 rather than
/// 200/201 because the event is committed to the log but the projections that answer
/// reads — the writing service's *and* view-service's — are still catching up; this
/// header is how the caller waits for its own write on the next read. Handlers spell
/// it out: `(StatusCode::ACCEPTED, [(X_VERSION, version)])`.
///
/// A response header rather than a body field because it is metadata about the
/// request, not an answer to it — and because every writer was otherwise obliged to
/// thread the version through a struct that had no other reason to exist. The client
/// reads it once, in its fetch wrapper, instead of at every call site.
pub const X_VERSION: HeaderName = HeaderName::from_static("x-version");
