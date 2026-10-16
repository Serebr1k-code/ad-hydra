use super::*;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::auth::AuthState,
};
use hydra_wire::v1::{self as proto, event::Payload};
async fn setup() -> (Traffic, std::path::PathBuf) {
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
