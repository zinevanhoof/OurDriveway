//! The route table resolves, including the bare namespace prefixes.
//!
//! `nest("/api/view/account", route("/", …))` reads as if it answers `/api/view/account`,
//! and whether it actually does is an axum detail rather than something the code says.
//! Getting it wrong 404s the whole profile screen and nothing else notices, so it is worth
//! one assertion per path.
//!
//! Not `#[ignore]`d and needs no database: the handlers are stubs, because what is under
//! test is the shape of the router, not what the shape returns.

use axum::{Router, body::Body, http::Request, routing::get};
use tower::ServiceExt;

/// The same nesting as `main.rs`, duplicated on purpose: `main` builds its router around
/// an `AppState` holding a live pool, so a test sharing that construction would be an
/// `#[ignore]`d test nobody runs. What is copied here is ten path strings — if they drift
/// from `main.rs`, this stops testing the thing it names, which is the trade.
fn app() -> Router {
    async fn ok() -> &'static str {
        "ok"
    }

    Router::new()
        .nest(
            "/api/view/account",
            Router::new().route("/", get(ok)).route("/wallet", get(ok)),
        )
        .nest(
            "/api/view/host",
            Router::new()
                .route("/spots", get(ok))
                .route("/spots/{id}", get(ok))
                .route("/balance", get(ok)),
        )
        .nest(
            "/api/view/renter",
            Router::new()
                .route("/bookings", get(ok))
                .route("/bookings/next", get(ok))
                .route("/bookings/{id}", get(ok)),
        )
        .nest(
            "/api/view/public",
            Router::new()
                .route("/spots/nearby", get(ok))
                .route("/spots/{id}", get(ok)),
        )
}

async fn status(path: &str) -> u16 {
    app()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Every path `apps/frontend/src/api/viewApi.ts` asks for.
#[tokio::test]
async fn every_route_resolves() {
    for path in [
        "/api/view/account",
        "/api/view/account/wallet",
        "/api/view/host/spots",
        "/api/view/host/spots/018f0000-0000-7000-8000-000000000000",
        "/api/view/host/balance",
        "/api/view/renter/bookings",
        "/api/view/renter/bookings/next",
        "/api/view/renter/bookings/018f0000-0000-7000-8000-000000000000",
        "/api/view/public/spots/nearby",
        "/api/view/public/spots/018f0000-0000-7000-8000-000000000000",
    ] {
        assert_eq!(status(path).await, 200, "{path} must resolve");
    }
}

/// `/spots/nearby` overlaps `/spots/{id}`, and the literal has to win.
///
/// If it did not, the map would ask for a spot whose id is the string "nearby" and get a
/// 404 that looks like an empty map. Same for `/bookings/next`.
#[tokio::test]
async fn a_literal_segment_beats_the_parameter_beside_it() {
    assert_eq!(status("/api/view/public/spots/nearby").await, 200);
    assert_eq!(status("/api/view/renter/bookings/next").await, 200);
}

/// The paths the four namespaces replaced are gone, and staying gone is the point.
///
/// A route left mounted alongside its replacement is a second way to read the same rows,
/// under the old rule — which for `/api/view/bookings/{id}` was the
/// `(renter_id = $2 OR host_id = $2)` disjunction this redesign split.
#[tokio::test]
async fn the_replaced_paths_are_not_still_mounted() {
    for path in [
        "/api/view/me",
        "/api/view/me/spots",
        "/api/view/me/bookings",
        "/api/view/me/wallet",
        "/api/view/me/balance",
        "/api/view/spots/nearby",
        "/api/view/spots/018f0000-0000-7000-8000-000000000000",
        "/api/view/spots/018f0000-0000-7000-8000-000000000000/manage",
        "/api/view/bookings/018f0000-0000-7000-8000-000000000000",
    ] {
        assert_eq!(status(path).await, 404, "{path} must be gone");
    }
}
