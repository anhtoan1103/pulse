//! CORS policy tests — docs/pulse-security.md #6: never `*` in production,
//! only the dashboard's own origin.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{TEST_FRONTEND_URL, auth_config};
use pulse_backend::{
    app::{self, AppState},
    db::MIGRATOR,
};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrator = "MIGRATOR")]
async fn allows_only_the_configured_frontend_origin(pool: PgPool) {
    let router = app::router(AppState::new(pool, TEST_FRONTEND_URL, &auth_config()));

    // A fixed allowed origin (not "mirror the request") always echoes that
    // *same* configured value, regardless of what `Origin` a request claims
    // — a non-browser client can put anything in that header, so the real
    // security boundary is that a genuine browser on evil.example would see
    // this response declare pulse.test as allowed, which doesn't match the
    // page's own true origin, and refuse to let its script read it.
    // What the server must never do is reflect the caller's own claimed
    // origin back (that would allow any site to read the response).
    for origin in [TEST_FRONTEND_URL, "https://evil.example"] {
        let req = Request::get("/health")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            TEST_FRONTEND_URL,
            "must always declare the configured origin, never reflect the caller's ({origin})"
        );
    }

    // No Origin header at all (a same-origin or non-browser request) still
    // works — CORS headers are only relevant to cross-origin browser fetches.
    let no_origin = Request::get("/health").body(Body::empty()).unwrap();
    let res = router.oneshot(no_origin).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn preflight_allows_the_methods_and_headers_the_dashboard_needs(pool: PgPool) {
    let router = app::router(AppState::new(pool, TEST_FRONTEND_URL, &auth_config()));

    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/endpoints")
        .header(header::ORIGIN, TEST_FRONTEND_URL)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PATCH")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type",
        )
        .body(Body::empty())
        .unwrap();
    let res = router.oneshot(preflight).await.unwrap();

    assert_eq!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        TEST_FRONTEND_URL
    );
    let allowed_methods = res
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_METHODS)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(allowed_methods.contains("PATCH"), "{allowed_methods}");
    let allowed_headers = res
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        allowed_headers.to_lowercase().contains("authorization"),
        "{allowed_headers}"
    );
}
