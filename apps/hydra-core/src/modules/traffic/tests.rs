use super::*;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::auth::AuthState,
};
use hydra_wire::v1::{self as proto, event::Payload};
pub(super) async fn setup() -> (Traffic, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("hydra-traffic-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(path.join("db")).await.unwrap();
    let crypto = Crypto::load(&path.join("key")).unwrap();
    (
        Traffic::open(AuthState::new(db, crypto, "https://hydra.test".into()))
            .await
            .unwrap(),
        path,
    )
}
#[tokio::test]
async fn newest_sessions_page_without_duplicates_when_new_connections_arrive() {
    let (t, path) = setup().await;
    let batch = |indices: std::ops::Range<usize>| proto::EventBatch {
        events: indices
            .map(|i| proto::Event {
                event_id: format!("open{i}"),
                flow_id: format!("flow{i}"),
                service_id: "svc".into(),
                payload: Some(Payload::FlowOpened(proto::FlowOpened {
                    transport: 1,
                    ..Default::default()
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    t.ingest(batch(0..3)).await.unwrap();
    let first = t
        .flows(serde_json::from_value(serde_json::json!({"newest_first":true,"limit":2})).unwrap())
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|v| v["flow_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["flow2", "flow1"]
    );
    t.ingest(batch(3..4)).await.unwrap();
    let next = t
        .flows(
            serde_json::from_value(
                serde_json::json!({"newest_first":true,"after":first.next_cursor,"limit":2}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0]["flow_id"], "flow0");
    assert!(next.next_cursor.is_none());
    assert_eq!(
        t.flows(Query::default()).await.unwrap().items[0]["flow_id"],
        "flow0"
    );
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn persistent_ingest_deduplicates_and_preserves_typed_fields() {
    let (t, path) = setup().await;
    let e = proto::Event {
        event_id: "e".into(),
        service_id: "s".into(),
        flow_id: "f".into(),
        payload: Some(Payload::Message(proto::ParsedMessage {
            message_id: "m".into(),
            fields: Some(proto::FieldObject {
                fields: std::collections::HashMap::from([(
                    "counter".into(),
                    proto::FieldValue {
                        value: Some(proto::field_value::Value::UintValue(u64::MAX)),
                    },
                )]),
            }),
            ..Default::default()
        })),
        ..Default::default()
    };
    let batch = proto::EventBatch {
        events: vec![e],
        ..Default::default()
    };
    assert_eq!(t.ingest(batch.clone()).await.unwrap().accepted, 1);
    assert_eq!(t.ingest(batch).await.unwrap().accepted, 0);
    let m = t.message("m".into()).await.unwrap().unwrap();
    assert_eq!(m["fields"]["counter"]["value"], u64::MAX.to_string());
    assert_eq!(
        t.messages("f".into(), Query::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn oversized_batches_and_rows_rejected_without_partial_commit() {
    let (t, path) = setup().await;
    assert!(
        t.ingest(proto::EventBatch {
            events: vec![proto::Event::default(); 257],
            ..Default::default()
        })
        .await
        .is_err()
    );
    let event = proto::Event {
        event_id: "e".into(),
        payload: Some(Payload::Message(proto::ParsedMessage {
            message_id: "m".into(),
            protocol_id: "x".repeat(70000),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert!(
        t.ingest(proto::EventBatch {
            events: vec![event],
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert!(t.message("m".into()).await.unwrap().is_none());
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn explorer_filters_share_message_predicate_and_stable_pagination() {
    let (t, path) = setup().await;
    t.auth
        .db
        .migrate(
            11,
            include_str!("../../../../../migrations/0011_flag_triage.sql"),
        )
        .await
        .unwrap();
    t.auth
        .db
        .migrate(
            12,
            include_str!("../../../../../migrations/0012_traffic_search.sql"),
        )
        .await
        .unwrap();
    let mut events = Vec::new();
    for i in 0..3 {
        let flow = format!("flow{i}");
        let mut timestamp = proto::Event::default().occurred_at.unwrap_or_default();
        timestamp.seconds = 100 + i;
        let event = |suffix: &str, payload| proto::Event {
            event_id: format!("{flow}-{suffix}"),
            flow_id: flow.clone(),
            service_id: "svc".into(),
            occurred_at: Some(timestamp),
            payload: Some(payload),
        };
        events.push(event(
            "open",
            Payload::FlowOpened(proto::FlowOpened {
                transport: 1,
                client: Some(proto::Endpoint {
                    ip: "192.0.2.1".into(),
                    port: 123,
                }),
                upstream: Some(proto::Endpoint {
                    ip: "192.0.2.2".into(),
                    port: 80,
                }),
            }),
        ));
        for direction in [1, 2] {
            events.push(event(
                &format!("m{direction}"),
                Payload::Message(proto::ParsedMessage {
                    message_id: format!("{flow}-m{direction}"),
                    direction,
                    fields: Some(proto::FieldObject {
                        fields: std::collections::HashMap::from([(
                            "http.path".into(),
                            proto::FieldValue {
                                value: Some(proto::field_value::Value::StringValue(
                                    if direction == 1 {
                                        "/request"
                                    } else {
                                        "/response"
                                    }
                                    .into(),
                                )),
                            },
                        )]),
                    }),
                    ..Default::default()
                }),
            ));
        }
        if i == 1 {
            events.push(event(
                "hit",
                Payload::RuleHit(proto::RuleHit {
                    rule_id: "rule1".into(),
                    ..Default::default()
                }),
            ));
        }
    }
    t.ingest(proto::EventBatch {
        events,
        ..Default::default()
    })
    .await
    .unwrap();
    let q = Query {
        service_id: Some("svc".into()),
        source_ip: Some("192.0.2.1".into()),
        destination_ip: Some("192.0.2.2".into()),
        transport: Some(1),
        direction: Some(2),
        field_path: Some("http.path".into()),
        field_value: Some("/response".into()),
        since_ms: Some(100000),
        until_ms: Some(102000),
        limit: Some(2),
        ..Default::default()
    };
    let first = t.flows(q).await.unwrap();
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.unwrap();
    let last = t
        .flows(Query {
            after: Some(cursor),
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.items[0]["flow_id"], "flow2");
    assert!(last.next_cursor.is_none());
    let wrong_direction = t
        .flows(Query {
            direction: Some(1),
            field_path: Some("http.path".into()),
            field_value: Some("/response".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(wrong_direction.items.is_empty());
    let hit = t
        .flows(Query {
            matched_rule: Some("rule1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(hit.items.len(), 1);
    assert_eq!(hit.items[0]["flow_id"], "flow1");
    let messages = t
        .messages(
            "flow1".into(),
            Query {
                direction: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(messages.items.len(), 1);
    assert_eq!(messages.items[0]["direction"], 2);
    assert_eq!(
        t.flows(Query {
            has_flag: Some(false),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len(),
        3
    );
    t.auth.db.run(|db| {
        db.execute("INSERT INTO flags(id,digest,category,value,initiated_by,created_at) VALUES('flag1','digest','own_exposed','secret','proxy',0)",[])?;
        db.execute("INSERT INTO flag_events(event_id,flag_id,flow_id,service_id) VALUES('critical-finding','flag1','flow2','svc')",[])?;
        Ok(())
    }).await.unwrap();
    let flagged = t
        .flows(Query {
            has_flag: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(flagged.items.len(), 1);
    assert_eq!(flagged.items[0]["flow_id"], "flow2");
    assert_eq!(flagged.items[0]["has_flag"], true);
    assert!(
        !serde_json::to_string(&flagged.items)
            .unwrap()
            .contains("secret")
    );
    let flow = t.flow("flow1".into()).await.unwrap().unwrap();
    assert_eq!(flow["rule_hits"][0]["rule_id"], "rule1");
    assert_eq!(flow["findings_truncated"], false);
    assert_eq!(
        t.flow("flow2".into()).await.unwrap().unwrap()["has_flag"],
        true
    );
    // SQL syntax in user input is compared literally; never executed.
    assert!(
        t.flows(Query {
            field_path: Some("http.path' OR 1=1 --".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .is_empty()
    );
    assert!(
        t.flows(Query {
            field_value: Some("x".into()),
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert!(
        t.flows(Query {
            field_path: Some("a.".repeat(17)),
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert!(
        t.flows(Query {
            limit: Some(101),
            ..Default::default()
        })
        .await
        .is_err()
    );
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn bounded_header_fields_commit_before_live_publication_without_losing_duplicates() {
    use hydra_domain::FieldValue as V;
    use std::collections::{BTreeMap, HashMap};
    let (traffic, path) = setup().await;
    let mut live = traffic.events.subscribe();
    assert_eq!(traffic.events.receiver_count(), 1);
    let event_id = format!("headers-event-{}", uuid::Uuid::new_v4());
    let headers = V::List(
        (0..128)
            .map(|_| {
                V::Object(BTreeMap::from([
                    ("name".into(), V::String("X-Duplicate".into())),
                    ("value".into(), V::String("\t".repeat(8))),
                ]))
            })
            .collect(),
    );
    let fields = HashMap::from([
        (
            "http.headers".into(),
            hydra_wire::rules::value_to_wire(&headers),
        ),
        (
            "http.trailers".into(),
            hydra_wire::rules::value_to_wire(&headers),
        ),
        (
            "http.body_text".into(),
            hydra_wire::rules::value_to_wire(&V::String("\0".repeat(1024))),
        ),
    ]);
    let accepted = traffic
        .ingest(proto::EventBatch {
            events: vec![proto::Event {
                event_id: event_id.clone(),
                flow_id: "flow".into(),
                service_id: "service".into(),
                payload: Some(Payload::Message(proto::ParsedMessage {
                    message_id: "headers-message".into(),
                    fields: Some(proto::FieldObject { fields }),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        accepted.accepted, 1,
        "event id: {event_id}, duplicates: {}",
        accepted.duplicates
    );
    assert_eq!(traffic.events.receiver_count(), 1);
    let notice = live.try_recv().unwrap_or_else(|error| {
        panic!(
            "No live event (receiver count {}, accepted {}): {error}",
            traffic.events.receiver_count(),
            accepted.accepted
        )
    });
    let stored = traffic
        .message("headers-message".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice, stored);
    assert_eq!(
        stored["fields"]["http.headers"]["value"]
            .as_array()
            .unwrap()
            .len(),
        128
    );
    assert_eq!(
        stored["fields"]["http.trailers"]["value"][127]["value"]["name"]["value"],
        "X-Duplicate"
    );
    assert!(serde_json::to_vec(&stored).unwrap().len() < 65536);
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn blocked_visibility_is_applied_before_pagination_and_ignores_shadow() {
    let (t, path) = setup().await;
    let mut events = Vec::new();
    for (i, mode, action) in [(0, 2, 3), (1, 1, 3), (2, 2, 2), (3, 2, 4), (4, 2, 6)] {
        let flow = format!("visibility{i}");
        events.push(proto::Event {
            event_id: format!("visibility-open{i}"),
            flow_id: flow.clone(),
            service_id: "svc".into(),
            payload: Some(Payload::FlowOpened(Default::default())),
            ..Default::default()
        });
        events.push(proto::Event {
            event_id: format!("visibility-message{i}"),
            flow_id: flow.clone(),
            service_id: "svc".into(),
            payload: Some(Payload::Message(proto::ParsedMessage {
                message_id: format!("visibility-m{i}"),
                ..Default::default()
            })),
            ..Default::default()
        });
        events.push(proto::Event {
            event_id: format!("visibility-hit{i}"),
            flow_id: flow,
            service_id: "svc".into(),
            payload: Some(Payload::RuleHit(proto::RuleHit {
                mode,
                action,
                blocked: i == 4,
                message_id: format!("visibility-m{i}"),
                ..Default::default()
            })),
            ..Default::default()
        });
    }
    t.ingest(proto::EventBatch {
        events,
        ..Default::default()
    })
    .await
    .unwrap();
    let query = |after| {
        serde_json::from_value(serde_json::json!({"include_blocked":false,"limit":1,"after":after}))
            .unwrap()
    };
    let first = t.flows(query(None::<i64>)).await.unwrap();
    assert_eq!(first.items[0]["flow_id"], "visibility1");
    assert_eq!(first.items[0]["blocked"], false);
    let second = t.flows(query(first.next_cursor)).await.unwrap();
    assert_eq!(second.items[0]["flow_id"], "visibility2");
    assert!(second.next_cursor.is_none());
    assert_eq!(t.flows(Query::default()).await.unwrap().items.len(), 5);
    assert_eq!(
        t.flow("visibility0".into()).await.unwrap().unwrap()["blocked"],
        true
    );
    assert!(
        t.messages("visibility0".into(), query(None))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        t.messages("visibility0".into(), Query::default())
            .await
            .unwrap()
            .items[0]["blocked"],
        true
    );
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn favorite_decoded_rows_survive_restart_with_finite_live_messages() {
    let (traffic, path) = setup().await;
    let id = uuid::Uuid::new_v4().to_string();
    traffic
        .ingest(proto::EventBatch {
            events: vec![proto::Event {
                event_id: "favorite-open".into(),
                flow_id: id.clone(),
                service_id: "svc".into(),
                payload: Some(Payload::FlowOpened(proto::FlowOpened {
                    transport: 1,
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    traffic
        .auth
        .db
        .run({
            let id = id.clone();
            move |db| {
                db.execute(
                    "INSERT INTO traffic_favorites(flow_id,raw_history_state) VALUES(?1,'pending')",
                    [id],
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();
    let messages = (0..140)
        .map(|i| proto::Event {
            event_id: format!("favorite-event-{i}"),
            flow_id: id.clone(),
            service_id: "svc".into(),
            payload: Some(Payload::Message(proto::ParsedMessage {
                message_id: format!("favorite-message-{i}"),
                protocol_id: "raw".into(),
                ..Default::default()
            })),
            ..Default::default()
        })
        .collect();
    traffic
        .ingest(proto::EventBatch {
            events: messages,
            ..Default::default()
        })
        .await
        .unwrap();
    traffic
        .ingest(proto::EventBatch {
            events: vec![proto::Event {
                event_id: "ordinary-open".into(),
                flow_id: "ordinary-flow".into(),
                service_id: "svc".into(),
                payload: Some(Payload::FlowOpened(proto::FlowOpened::default())),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let flow = traffic.flow(id.clone()).await.unwrap().unwrap();
    assert_eq!(flow["favorite"], true);
    assert_eq!(flow["decoded_incomplete"], true);
    let (count, bytes) = traffic
        .auth
        .db
        .run({
            let id = id.clone();
            move |db| {
                Ok(db.query_row(
                    "SELECT message_count,bytes FROM traffic_favorites WHERE flow_id=?1",
                    [id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(count, 128);
    assert!(bytes < 900000);
    let restarted = Traffic::open(traffic.auth.clone()).await.unwrap();
    let page = restarted
        .flows(serde_json::from_value(serde_json::json!({"favorite":true})).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0]["flow_id"], id);
    assert_eq!(page.items[0]["raw_history_state"], "pending");
    assert_eq!(page.items[0]["decoded_incomplete"], true);
    assert_eq!(
        restarted
            .messages(
                id.clone(),
                Query {
                    limit: Some(100),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .items
            .len(),
        100
    );
    restarted
        .auth
        .db
        .run({
            let id = id.clone();
            move |db| {
                db.execute("DELETE FROM traffic_favorites WHERE flow_id=?1", [id])?;
                Ok(())
            }
        })
        .await
        .unwrap();
    assert_eq!(
        restarted.flow(id).await.unwrap().unwrap()["favorite"],
        false
    );
    drop(restarted);
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn favorite_api_auth_origin_failed_raw_pin_and_audit_are_explicit_and_idempotent() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (traffic, path) = setup().await;
    crate::modules::users::create(
        &traffic.auth.db,
        &traffic.auth.crypto,
        "owner".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let token = traffic
        .auth
        .login(
            "owner".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap()
        .response
        .access_token;
    let id = uuid::Uuid::new_v4().to_string();
    traffic
        .ingest(proto::EventBatch {
            events: vec![proto::Event {
                event_id: "favorite-api-open".into(),
                flow_id: id.clone(),
                service_id: "svc".into(),
                payload: Some(Payload::FlowOpened(proto::FlowOpened::default())),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let app = super::diagnostics::router(super::diagnostics::Diagnostics {
        auth: traffic.auth.clone(),
        socket: path.join("missing.sock"),
        traffic: Some(traffic.clone()),
    });
    let request = |authenticated: bool, origin: &str| {
        let mut request = Request::put(format!("/api/v1/traffic/flows/{id}/favorite"))
            .header("origin", origin)
            .header("content-type", "application/json");
        if authenticated {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request.body(Body::from("{\"favorite\":true}")).unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(request(false, "https://hydra.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(request(true, "https://other.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request(true, "https://hydra.test"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "favorite_pending");
    }
    let unpin = Request::put(format!("/api/v1/traffic/flows/{id}/favorite"))
        .header("origin", "https://hydra.test")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from("{\"favorite\":false}"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(unpin).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        traffic.flow(id.clone()).await.unwrap().unwrap()["raw_history_state"],
        "removing"
    );
    assert_eq!(
        app.clone()
            .oneshot(request(true, "https://hydra.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let restarted = Traffic::open(traffic.auth.clone()).await.unwrap();
    assert_eq!(
        restarted.flow(id.clone()).await.unwrap().unwrap()["raw_history_state"],
        "pending"
    );
    let (count,failed)=traffic.auth.db.run({let id=id.clone();move |db| {Ok((db.query_row("SELECT count(*) FROM traffic_favorites WHERE flow_id=?1",[&id],|r|r.get::<_,i64>(0))?,db.query_row("SELECT count(*) FROM audit WHERE target=?1 AND action='traffic.favorite.result' AND success=0 AND actor_kind='user'",[id],|r|r.get::<_,i64>(0))?))}}).await.unwrap();
    assert_eq!(count, 1);
    assert_eq!(failed, 4);
    let saved = traffic.flow(id).await.unwrap().unwrap();
    assert_eq!(saved["favorite"], true);
    assert_eq!(saved["raw_history_state"], "pending");
    drop(app);
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn expired_filter_preview_stream_explains_missing_retained_history() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (traffic, path) = setup().await;
    crate::modules::users::create(
        &traffic.auth.db,
        &traffic.auth.crypto,
        "preview-owner".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let token = traffic
        .auth
        .login(
            "preview-owner".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap()
        .response
        .access_token;
    let app = super::router(traffic.clone());
    for route in [
        "/api/v1/traffic/flows/expired-stream",
        "/api/v1/traffic/messages/expired-message",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(route)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "traffic_not_retained");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("retained")
        );
    }
    drop(app);
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn saved_and_live_text_messages_share_stable_capture_order_at_equal_timestamps() {
    let (t, path) = setup().await;
    let mut events = t.events.subscribe();
    t.ingest(proto::EventBatch {
        events: (0..3)
            .map(|i| proto::Event {
                event_id: format!("terminal-event-{i}"),
                flow_id: "terminal-flow".into(),
                service_id: "nc".into(),
                payload: Some(Payload::Message(proto::ParsedMessage {
                    message_id: format!("terminal-message-{i}"),
                    protocol_id: "utf8".into(),
                    direction: if i == 1 { 1 } else { 2 },
                    message_type: "text".into(),
                    ..Default::default()
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    })
    .await
    .unwrap();
    let mut live = Vec::new();
    for _ in 0..3 {
        live.push(
            events.recv().await.unwrap()["stream_sequence"]
                .as_i64()
                .unwrap(),
        );
    }
    assert!(live.windows(2).all(|pair| pair[0] < pair[1]));
    let page = t
        .messages(
            "terminal-flow".into(),
            serde_json::from_value(serde_json::json!({"newest_first":true})).unwrap(),
        )
        .await
        .unwrap();
    let mut saved = page
        .items
        .iter()
        .map(|m| m["stream_sequence"].as_i64().unwrap())
        .collect::<Vec<_>>();
    saved.reverse();
    assert_eq!(saved, live);
    drop(t);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn saved_direction_scope_uses_message_metadata_for_legacy_decoder_fields() {
    let (traffic, path) = setup().await;
    traffic
        .ingest(proto::EventBatch {
            events: vec![proto::Event {
                event_id: "legacy-side-event".into(),
                flow_id: "legacy-side-flow".into(),
                service_id: "service".into(),
                payload: Some(Payload::Message(proto::ParsedMessage {
                    message_id: "legacy-side-message".into(),
                    direction: 2,
                    fields: Some(proto::FieldObject {
                        fields: std::collections::HashMap::new(),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let fields = super::saved_fields(&traffic.auth.db, "legacy-side-message".into())
        .await
        .unwrap();
    assert_eq!(
        fields.get("direction"),
        Some(&hydra_domain::FieldValue::String("server_to_client".into()))
    );
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn flow_detail_exposes_only_allowlisted_decoder_warnings() {
    let (traffic, path) = setup().await;
    traffic
        .ingest(proto::EventBatch {
            events: vec![
                proto::Event {
                    event_id: "decoder-flow-open".into(),
                    flow_id: "decoder-flow".into(),
                    service_id: "svc".into(),
                    payload: Some(Payload::FlowOpened(proto::FlowOpened {
                        transport: 1,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                proto::Event {
                    event_id: "decoder-warning".into(),
                    flow_id: "decoder-flow".into(),
                    service_id: "svc".into(),
                    payload: Some(Payload::Loss(proto::EventLoss {
                        count: 1,
                        reason: "http_response_decoder_disabled".into(),
                    })),
                    ..Default::default()
                },
                proto::Event {
                    event_id: "unknown-warning".into(),
                    flow_id: "decoder-flow".into(),
                    service_id: "svc".into(),
                    payload: Some(Payload::Loss(proto::EventLoss {
                        count: 1,
                        reason: "private decoder detail".into(),
                    })),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .await
        .unwrap();

    let detail = traffic.flow("decoder-flow".into()).await.unwrap().unwrap();
    assert_eq!(
        detail["decoder_warnings"],
        serde_json::json!(["http_response_decoder_disabled"])
    );
    assert!(!detail.to_string().contains("private decoder detail"));
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}
