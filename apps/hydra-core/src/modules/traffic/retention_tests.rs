use super::repository::Traffic;
use hydra_wire::v1::{self as p, event::Payload};

use super::tests::setup;
fn opening(id: &str, service: &str) -> p::Event {
    p::Event {
        event_id: format!("open-{id}"),
        flow_id: id.into(),
        service_id: service.into(),
        payload: Some(Payload::FlowOpened(p::FlowOpened {
            transport: 1,
            ..Default::default()
        })),
        ..Default::default()
    }
}
fn message(id: &str, service: &str) -> p::Event {
    p::Event {
        event_id: format!("event-{id}"),
        flow_id: id.into(),
        service_id: service.into(),
        payload: Some(Payload::Message(p::ParsedMessage {
            message_id: format!("message-{id}"),
            fields: Some(p::FieldObject {
                fields: std::collections::HashMap::from([(
                    "utf8.text".into(),
                    p::FieldValue {
                        value: Some(p::field_value::Value::StringValue(
                            "Hello world\n".repeat(2000),
                        )),
                    },
                )]),
            }),
            ..Default::default()
        })),
        ..Default::default()
    }
}
#[tokio::test]
async fn decoded_rows_are_compressed_at_three_and_api_fields_roundtrip() {
    let (traffic, path) = setup().await;
    traffic
        .ingest(p::EventBatch {
            events: vec![opening("a", "aquarius"), message("a", "aquarius")],
            ..Default::default()
        })
        .await
        .unwrap();
    traffic
        .auth
        .db
        .run(|db| {
            let blob: Vec<u8> = db.query_row(
                "SELECT data FROM traffic_messages_zstd WHERE id='message-a'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(&blob[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
            assert!(blob.len() < 1000);
            let text: String = db.query_row(
                "SELECT data FROM traffic_messages WHERE id='message-a'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                blob,
                hydra_compression::encode_at_level(text.as_bytes(), 3)?
            );
            let used: i64 = db.query_row(
                "SELECT bytes FROM traffic_service_budget WHERE service_id='aquarius'",
                [],
                |r| r.get(0),
            )?;
            assert!(used < 3000);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        traffic.message("message-a".into()).await.unwrap().unwrap()["fields"]["utf8.text"]["value"],
        "Hello world\n".repeat(2000)
    );
    let reopened = Traffic::open(traffic.auth.clone()).await.unwrap();
    assert!(
        reopened
            .message("message-a".into())
            .await
            .unwrap()
            .is_some()
    );
    drop(reopened);
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

fn closing(id: &str, service: &str) -> p::Event {
    p::Event {
        event_id: format!("close-{id}"),
        flow_id: id.into(),
        service_id: service.into(),
        payload: Some(Payload::FlowClosed(p::FlowClosed {
            reason: "eof".into(),
            ..Default::default()
        })),
        ..Default::default()
    }
}
#[tokio::test]
async fn service_rotation_preserves_other_services_and_favorites_and_removes_whole_sessions() {
    let (traffic, path) = setup().await;
    for (id, service) in [
        ("a1", "aquarius"),
        ("a2", "aquarius"),
        ("a3", "aquarius"),
        ("b1", "ark"),
    ] {
        traffic
            .ingest(p::EventBatch {
                events: vec![
                    opening(id, service),
                    message(id, service),
                    closing(id, service),
                ],
                ..Default::default()
            })
            .await
            .unwrap();
    }
    traffic.auth.db.run(|db|{
        db.execute("INSERT INTO traffic_favorites(flow_id,raw_history_state) VALUES('a1','saved')",[])?;
        let before:i64=db.query_row("SELECT bytes FROM traffic_service_budget WHERE service_id='aquarius'",[],|r|r.get(0))?;
        let ark:i64=db.query_row("SELECT bytes FROM traffic_service_budget WHERE service_id='ark'",[],|r|r.get(0))?;
        let tx=db.transaction()?;super::retention::enforce(&tx,before-1)?;tx.commit()?;
        assert_eq!(db.query_row("SELECT count(*) FROM traffic_flows WHERE id IN ('a1','a3','b1')",[],|r|r.get::<_,i64>(0))?,3);
        assert_eq!(db.query_row("SELECT count(*) FROM traffic_flows WHERE id='a2'",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT count(*) FROM traffic_messages WHERE flow_id='a2'",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT count(*) FROM traffic_events WHERE flow_id='a2'",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT bytes FROM traffic_service_budget WHERE service_id='ark'",[],|r|r.get::<_,i64>(0))?,ark);
        let counted:i64=db.query_row("SELECT sum(bytes) FROM traffic_service_budget",[],|r|r.get(0))?;
        let actual:i64=db.query_row("SELECT (SELECT COALESCE(sum(length(data)),0) FROM traffic_events_zstd)+(SELECT COALESCE(sum(length(data)),0) FROM traffic_messages_zstd)+(SELECT COALESCE(sum(length(data)),0) FROM traffic_flows_zstd)",[],|r|r.get(0))?;
        assert_eq!(actual,counted);
        assert_eq!(actual,db.query_row("SELECT bytes FROM traffic_budget",[],|r|r.get::<_,i64>(0))?);
        Ok(())
    }).await.unwrap();
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn active_tail_rotation_keeps_the_session_card_and_reports_incomplete_history() {
    let (traffic, path) = setup().await;
    let mut events = vec![opening("active", "aquarius")];
    for i in 0..100 {
        let mut event = message("active", "aquarius");
        event.event_id = format!("event-{i}");
        if let Some(Payload::Message(m)) = &mut event.payload {
            m.message_id = format!("message-{i}")
        }
        events.push(event);
    }
    traffic
        .ingest(p::EventBatch {
            events,
            ..Default::default()
        })
        .await
        .unwrap();
    traffic
        .auth
        .db
        .run(|db| {
            let before: i64 = db.query_row(
                "SELECT bytes FROM traffic_service_budget WHERE service_id='aquarius'",
                [],
                |r| r.get(0),
            )?;
            let tx = db.transaction()?;
            super::retention::enforce(&tx, before / 2)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    let flow = traffic.flow("active".into()).await.unwrap().unwrap();
    assert_eq!(flow["decoded_incomplete"], true);
    assert!(
        !traffic
            .messages("active".into(), Default::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn legacy_json_migrates_and_recovers_cards_for_retained_orphan_messages() {
    use crate::{
        infrastructure::{crypto::Crypto, db::Database},
        modules::auth::AuthState,
    };
    let path = std::env::temp_dir().join(format!("hydra-decoded-legacy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(path.join("db")).await.unwrap();
    db.run(|db|{
  let open=serde_json::json!({"kind":"flow_opened","flow_id":"legacy","service_id":"aquarius","occurred_ms":123,"client":{"ip":"127.0.0.1","port":1234},"transport":1});
  let message=serde_json::json!({"kind":"message","flow_id":"legacy","service_id":"aquarius","event_id":"legacy-message-event","message_id":"legacy-message","direction":2,"fields":{"utf8.text":{"type":"string","value":"saved answer"}}});
  let mut missing=message.clone();missing["message_id"]="legacy-second".into();missing["event_id"]="legacy-second-event".into();missing["fields"]["utf8.text"]["value"]="recovered second answer".into();
  let closed=serde_json::json!({"kind":"flow_closed","flow_id":"legacy","service_id":"aquarius","reason":"eof"});
  for (id,kind,data) in [("legacy-open","flow_opened",open),("legacy-message-event","message",message.clone()),("legacy-second-event","message",missing),("legacy-close","flow_closed",closed)] {db.execute("INSERT INTO traffic_events(event_id,flow_id,kind,data) VALUES(?1,'legacy',?2,?3)",rusqlite::params![id,kind,data.to_string()])?;}
  db.execute("INSERT INTO traffic_messages(id,flow_id,service_id,occurred_ms,data) VALUES('legacy-message','legacy','aquarius',123,?1)",[message.to_string()])?;
  Ok(())
 }).await.unwrap();
    let traffic = Traffic::open(AuthState::new(
        db,
        Crypto::load(&path.join("key")).unwrap(),
        "https://hydra.test".into(),
    ))
    .await
    .unwrap();
    let recovered = traffic
        .messages("legacy".into(), Default::default())
        .await
        .unwrap();
    assert_eq!(
        recovered
            .items
            .iter()
            .map(|m| m["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["legacy-message", "legacy-second"]
    );
    assert_eq!(
        recovered.items[1]["fields"]["utf8.text"]["value"],
        "recovered second answer"
    );
    let flow = traffic.flow("legacy".into()).await.unwrap().unwrap();
    assert_eq!(flow["closed"]["reason"], "eof");
    assert_eq!(
        traffic
            .message("legacy-message".into())
            .await
            .unwrap()
            .unwrap()["fields"]["utf8.text"]["value"],
        "saved answer"
    );
    traffic
        .auth
        .db
        .run(|db| {
            let storage: String = db.query_row(
                "SELECT typeof(data) FROM traffic_messages_zstd WHERE id='legacy-message'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(storage, "blob");
            Ok(())
        })
        .await
        .unwrap();
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn event_only_active_raw_sessions_can_rotate_without_stalling_ingest() {
    let (traffic, path) = setup().await;
    let mut events = vec![opening("active-raw", "aquarius")];
    for i in 0..100 {
        events.push(p::Event {
            event_id: format!("raw-hit-{i}"),
            flow_id: "active-raw".into(),
            service_id: "aquarius".into(),
            payload: Some(Payload::RuleHit(p::RuleHit {
                rule_id: "raw-alert".into(),
                mode: 1,
                action: 2,
                ..Default::default()
            })),
            ..Default::default()
        });
    }
    traffic
        .ingest(p::EventBatch {
            events,
            ..Default::default()
        })
        .await
        .unwrap();
    traffic
        .auth
        .db
        .run(|db| {
            let before: i64 = db.query_row(
                "SELECT bytes FROM traffic_service_budget WHERE service_id='aquarius'",
                [],
                |r| r.get(0),
            )?;
            let tx = db.transaction()?;
            super::retention::enforce(&tx, before / 2)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        traffic.flow("active-raw".into()).await.unwrap().unwrap()["decoded_incomplete"],
        true
    );
    traffic
        .ingest(p::EventBatch {
            events: vec![closing("active-raw", "aquarius")],
            ..Default::default()
        })
        .await
        .unwrap();
    drop(traffic);
    std::fs::remove_dir_all(path).unwrap();
}
