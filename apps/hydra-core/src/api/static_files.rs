use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
const ROOT: &str = include_str!("../../../../frontend/index.html");
pub fn router() -> Router {
    Router::new()
        .route("/", get(|| async { Html(ROOT) }))
        .route("/assets/{name}", get(asset))
}
async fn asset(Path(name): Path<String>) -> Response {
    let (mime, source) = match name.as_str() {
        "style.css" => (
            "text/css",
            include_str!("../../../../frontend/src/app/style.css"),
        ),
        "app.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/app/app.js"),
        ),
        "api.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/shared/api.js"),
        ),
        "view.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/shared/view.js"),
        ),
        "services.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/services/page.js"),
        ),
        "filters.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/filters/page.js"),
        ),
        "traffic.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/traffic/page.js"),
        ),
        "users.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/users/page.js"),
        ),
        "protocols.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/protocols/page.js"),
        ),
        "flags.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/flags/page.js"),
        ),
        "farm.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/farm/page.js"),
        ),
        "settings.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/features/settings/page.js"),
        ),
        "operations.js" => (
            "text/javascript",
            include_str!("../../../../frontend/src/generated/operations.js"),
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            ("content-type", mime),
            ("x-content-type-options", "nosniff"),
            ("cache-control", "no-cache"),
        ],
        source,
    )
        .into_response()
}
