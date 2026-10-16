use axum::{
    Router,
    extract::OriginalUri,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};

include!(concat!(env!("OUT_DIR"), "/frontend_assets.rs"));

pub fn router() -> Router {
    Router::new()
        .route("/", get(asset))
        .route("/assets/{*path}", get(asset))
}

async fn asset(OriginalUri(uri): OriginalUri) -> Response {
    // Compare the original encoded path exactly: no decoding or filesystem access.
    // Vue Router uses hash navigation, so only the root document needs serving.
    let Some((path, mime, bytes)) = ASSETS.iter().find(|(path, _, _)| *path == uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            ("content-type", *mime),
            ("x-content-type-options", "nosniff"),
            (
                "cache-control",
                if *path == "/" {
                    "no-cache"
                } else {
                    "public, max-age=31536000, immutable"
                },
            ),
        ],
        *bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_compiled_bundle_and_cache_headers() {
        let response = router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let html = String::from_utf8(
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("type=\"module\""));
        assert!(html.contains("/assets/"));
        assert!(html.contains("id=\"app\""));
        let js = ASSETS
            .iter()
            .find(|(path, _, _)| path.ends_with(".js") && html.contains(*path))
            .unwrap();
        assert!(
            html.contains(js.0),
            "index must reference its compiled script"
        );
        for (path, mime, source) in ASSETS.iter().filter(|(path, _, _)| *path != "/") {
            let response = router()
                .oneshot(Request::builder().uri(*path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], *mime);
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            assert_eq!(
                response.headers()["cache-control"],
                "public, max-age=31536000, immutable"
            );
            assert_eq!(
                to_bytes(response.into_body(), 32 * 1024 * 1024)
                    .await
                    .unwrap()
                    .as_ref(),
                *source
            );
        }
    }

    #[tokio::test]
    async fn refuses_unknown_assets_and_traversal() {
        for path in [
            "/assets/missing.js",
            "/assets/../index.html",
            "/assets/%2e%2e/index.html",
            "/assets/%2fetc/passwd",
            "/src/app/App.vue",
            "/filters",
        ] {
            let response = router()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}
