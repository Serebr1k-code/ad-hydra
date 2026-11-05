//! Durable, bounded favorite intents coordinated with the raw Proxy archive.
use super::diagnostics::{Diagnostics, actor};
use crate::{infrastructure::proxy_client, modules::audit};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use hydra_wire::v1::{self, proxy_diagnostics_client::ProxyDiagnosticsClient};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
impl Diagnostics {
    async fn favorite_changed(&self, id: &str, favorite: bool, raw: &str, incomplete: bool) {
        if let Some(traffic) = &self.traffic {
            let target = id.to_owned();
            let context=self.auth.db.run(move |db| {
                Ok(db.query_row("SELECT service_id,COALESCE((SELECT decoded_incomplete FROM traffic_favorites WHERE flow_id=?1),0) FROM traffic_flows WHERE id=?1",[target],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?)))?)
            }).await;
            if let Ok((service_id, saved_incomplete)) = context {
                let _=traffic.events.send(serde_json::json!({"kind":"favorite_changed","flow_id":id,"service_id":service_id,"favorite":favorite,"raw_history_state":raw,"decoded_incomplete":incomplete||saved_incomplete}));
            }
        }
    }
    /// Retry durable intents after either process restarts. Bounded by 128 slots.
    pub fn resume_favorites(&self, shutdown: tokio_util::sync::CancellationToken) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! { _=shutdown.cancelled()=>break,_=interval.tick()=>{} }
                let _guard = FAVORITE_MUTATION.lock().await;
                let rows=state.auth.db.run(|db| {
                    let mut stmt=db.prepare("SELECT flow_id,raw_history_state FROM traffic_favorites WHERE raw_history_state IN ('pending','removing') LIMIT 128")?;
                    Ok(stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?)
                }).await.unwrap_or_default();
                for (id, old) in rows {
                    let wanted = old != "removing";
                    let Ok(channel) = proxy_client::connect(&state.socket).await else {
                        break;
                    };
                    let result = ProxyDiagnosticsClient::new(channel)
                        .set_flow_favorite(favorite_request(v1::SetFlowFavoriteRequest {
                            flow_id: id.clone(),
                            favorite: wanted,
                            request_id: uuid::Uuid::new_v4().to_string(),
                            actor: Some(v1::Actor {
                                r#type: 2,
                                ..Default::default()
                            }),
                        }))
                        .await;
                    let Ok(reply) = result else {
                        continue;
                    };
                    let raw = reply.into_inner().state;
                    if raw == old {
                        continue;
                    }
                    let outcome=state.auth.db.run({let id=id.clone();let raw=raw.clone();move |db| {
                        let tx=db.transaction()?;
                        if wanted {tx.execute("UPDATE traffic_favorites SET raw_history_state=?2 WHERE flow_id=?1",rusqlite::params![id,raw])?;} else {tx.execute("DELETE FROM traffic_favorites WHERE flow_id=?1",[&id])?;}
                        audit::append(&tx,&audit::Actor {id:"favorite-recovery".into(),kind:"system".into()},"traffic.favorite.recovered",&id,&uuid::Uuid::new_v4().to_string(),true,crate::modules::auth::now())?;
                        tx.commit()?;Ok(())
                    }}).await;
                    if outcome.is_ok() {
                        state.favorite_changed(&id, wanted, &raw, false).await;
                    }
                }
            }
        });
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FavoriteInput {
    pub favorite: bool,
}
#[derive(Serialize, ToSchema)]
pub struct FavoriteResult {
    pub flow_id: String,
    pub favorite: bool,
    pub raw_history_state: String,
    pub decoded_incomplete: bool,
}
static FAVORITE_MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[utoipa::path(put,path="/api/v1/traffic/flows/{id}/favorite",params(("id"=String,Path)),request_body=FavoriteInput,responses((status=200,body=FavoriteResult),(status=401),(status=403),(status=404),(status=409,description="Favorite capacity or retained decoded data exceeds archive bounds"),(status=503)),tag="traffic")]
pub(super) async fn favorite(
    state: State<Diagnostics>,
    headers: HeaderMap,
    path: Path<String>,
    input: Json<FavoriteInput>,
) -> Result<Json<FavoriteResult>, FavoriteError> {
    let audit_state = state.0.clone();
    let audit_headers = headers.clone();
    let target = path.0.clone();
    let result = favorite_impl(state, headers, path, input).await;
    if let Err(status) = result {
        if let Ok(who) = actor(&audit_state, &audit_headers, true).await {
            let _ = super::diagnostics::record(
                &audit_state,
                who,
                "traffic.favorite.failed",
                target,
                uuid::Uuid::new_v4().to_string(),
                false,
            )
            .await;
        }
        return Err(FavoriteError(status));
    }
    result.map_err(FavoriteError)
}
pub(super) struct FavoriteError(StatusCode);
impl axum::response::IntoResponse for FavoriteError {
    fn into_response(self) -> axum::response::Response {
        let (code, message) = match self.0 {
            StatusCode::CONFLICT => (
                "favorite_capacity_exhausted",
                "Favorite archive is full or the retained decoded data exceeds 128 messages / 900000 bytes; remove a favorite or choose a smaller flow.",
            ),
            StatusCode::SERVICE_UNAVAILABLE => (
                "favorite_pending",
                "Favorite could not be confirmed. A pending save or removal will be retried; refresh to check its state.",
            ),
            StatusCode::NOT_FOUND => ("flow_not_found", "Flow is no longer retained."),
            StatusCode::UNAUTHORIZED => ("authentication_required", "Authentication required."),
            StatusCode::FORBIDDEN => ("origin_rejected", "Request origin rejected."),
            _ => ("invalid_favorite", "Invalid favorite request."),
        };
        (
            self.0,
            Json(serde_json::json!({"error":{"code":code,"message":message}})),
        )
            .into_response()
    }
}
async fn favorite_impl(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FavoriteInput>,
) -> Result<Json<FavoriteResult>, StatusCode> {
    use rusqlite::OptionalExtension;
    let who = actor(&state, &headers, true).await?;
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let _guard = FAVORITE_MUTATION.lock().await;
    let request = uuid::Uuid::new_v4().to_string();
    let wanted = input.favorite;
    let existed=state.auth.db.run({let id=id.clone();let who=who.clone();let request=request.clone();move |db| {
        let tx=db.transaction()?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM traffic_flows WHERE id=?1)",[&id],|r|r.get(0))?;
        anyhow::ensure!(exists,"missing");
        let old:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM traffic_favorites WHERE flow_id=?1)",[&id],|r|r.get(0))?;
        if wanted && !old {
            let count:i64=tx.query_row("SELECT count(*) FROM traffic_favorites",[],|r|r.get(0))?;
            anyhow::ensure!(count<128,"capacity");
            let (bytes,messages)=favorite_size(&tx,&id)?;
            anyhow::ensure!(count<128 && bytes<=900000 && messages<=128,"capacity");
            tx.execute("INSERT INTO traffic_favorites(flow_id,raw_history_state) VALUES(?1,'pending')",[&id])?;
        }
        if wanted && old {tx.execute("UPDATE traffic_favorites SET raw_history_state='pending' WHERE flow_id=?1 AND raw_history_state='removing'",[&id])?;}
        if !wanted && old {tx.execute("UPDATE traffic_favorites SET raw_history_state='removing' WHERE flow_id=?1",[&id])?;}
        audit::append(&tx,&who,"traffic.favorite.request",&id,&request,true,crate::modules::auth::now())?;
        tx.commit()?;Ok(old)
    }}).await.map_err(|e|if e.to_string()=="missing" {StatusCode::NOT_FOUND}else if e.to_string()=="capacity" {StatusCode::CONFLICT}else{StatusCode::SERVICE_UNAVAILABLE})?;
    let result = async {
        let channel = proxy_client::connect(&state.socket)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        ProxyDiagnosticsClient::new(channel)
            .set_flow_favorite(favorite_request(v1::SetFlowFavoriteRequest {
                flow_id: id.clone(),
                favorite: wanted,
                request_id: request.clone(),
                actor: Some(v1::Actor {
                    r#type: 1,
                    user_id: who.id.clone(),
                    ..Default::default()
                }),
            }))
            .await
            .map(|v| v.into_inner().state)
            .map_err(|e| {
                if e.code() == tonic::Code::ResourceExhausted {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            })
    }
    .await;
    let capacity = result.as_ref().err() == Some(&StatusCode::CONFLICT);
    let success = result.is_ok();
    let raw = result
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| "pending".into());
    let incomplete = state
        .auth
        .db
        .run({
            let id = id.clone();
            let raw = raw.clone();
            move |db| {
                let tx = db.transaction()?;
                if success {
                    if wanted {
                        tx.execute(
                            "UPDATE traffic_favorites SET raw_history_state=?2 WHERE flow_id=?1",
                            rusqlite::params![id, raw],
                        )?;
                    } else {
                        tx.execute("DELETE FROM traffic_favorites WHERE flow_id=?1", [&id])?;
                    }
                } else if !existed && capacity {
                    tx.execute("DELETE FROM traffic_favorites WHERE flow_id=?1", [&id])?;
                }
                audit::append(
                    &tx,
                    &who,
                    "traffic.favorite.result",
                    &id,
                    &request,
                    success,
                    crate::modules::auth::now(),
                )?;
                let incomplete = tx
                    .query_row(
                        "SELECT decoded_incomplete FROM traffic_favorites WHERE flow_id=?1",
                        [&id],
                        |r| r.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false);
                tx.commit()?;
                Ok(incomplete)
            }
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    result?;
    state.favorite_changed(&id, wanted, &raw, incomplete).await;
    Ok(Json(FavoriteResult {
        flow_id: id,
        favorite: wanted,
        raw_history_state: raw,
        decoded_incomplete: incomplete,
    }))
}

fn favorite_request(
    value: v1::SetFlowFavoriteRequest,
) -> tonic::Request<v1::SetFlowFavoriteRequest> {
    let mut request = tonic::Request::new(value);
    request.set_timeout(std::time::Duration::from_secs(3));
    request
}

/// Bound decompression during admission: reject large flows before summing their
/// whole archive, and stop expanding records as soon as the favorite limit is hit.
fn favorite_size(tx: &rusqlite::Transaction<'_>, id: &str) -> anyhow::Result<(i64, i64)> {
    let messages: i64 = tx.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM traffic_messages_zstd WHERE flow_id=?1 LIMIT 129)",
        [id],
        |r| r.get(0),
    )?;
    anyhow::ensure!(messages <= 128, "capacity");
    let mut bytes = 0_i64;
    for (table, key) in [
        ("traffic_flows_zstd", "id"),
        ("traffic_messages_zstd", "flow_id"),
        ("traffic_events_zstd", "flow_id"),
    ] {
        let mut query = tx.prepare(&format!(
            "SELECT length(CAST(hydra_text(data) AS BLOB)) FROM {table} WHERE {key}=?1"
        ))?;
        let mut rows = query.query([id])?;
        while let Some(row) = rows.next()? {
            bytes += row.get::<_, i64>(0)?;
            anyhow::ensure!(bytes <= 900000, "capacity");
        }
    }
    Ok((bytes, messages))
}
