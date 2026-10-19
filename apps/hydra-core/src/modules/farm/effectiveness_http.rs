use super::{
    Farm,
    effectiveness_dto::*,
    http::{Error, actor},
};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};

#[utoipa::path(get, path="/api/v1/farm/effectiveness", params(
    ("service_id"=Option<String>, Query, description="Farm service ID; empty selects legacy targets"),
    ("window"=Option<String>, Query, description="1h, 24h (default), 7d, or retained; filters run creation time"),
    ("row_offset"=Option<u32>, Query), ("team_offset"=Option<u32>, Query)
), responses((status=200, body=EffectivenessMatrix),(status=400),(status=401)), tag="farm")]
pub(super) async fn effectiveness(
    State(farm): State<Farm>,
    headers: HeaderMap,
    Query(query): Query<EffectivenessQuery>,
) -> Result<Json<EffectivenessMatrix>, Error> {
    let actor = actor(&farm, &headers).await?;
    let window = query.window.clone().unwrap_or_else(|| "24h".into());
    if !["1h", "24h", "7d", "retained"].contains(&window.as_str())
        || query.row_offset > 20_000
        || query.team_offset > 20_000
        || query.service_id.as_ref().is_some_and(|s| s.len() > 64)
    {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "invalid effectiveness filters or page".into(),
        ));
    }
    Ok(Json(
        super::effectiveness_repository::read(&farm.auth.db, actor, query, window).await?,
    ))
}
