use super::*;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::{audit::Actor, auth::AuthState},
};
use hydra_domain::*;
pub(super) async fn setup() -> (Control, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("hydra-control-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(path.join("db")).await.unwrap();
    let crypto = Crypto::load(&path.join("key")).unwrap();
    (
        Control::open(AuthState::new(db, crypto, "https://hydra.test".into()))
            .await
            .unwrap(),
        path,
    )
}
fn actor(id: &str) -> Actor {
    Actor::user(id.into())
}
fn service() -> Service {
    Service {
        forwarding: None,
        id: "service".into(),
        transport: Transport::Tcp,
        listen_address: "127.0.0.1:21001".parse().unwrap(),
        upstream_address: "127.0.0.1:21002".parse().unwrap(),
        protocol_id: "http".into(),
        enabled: true,
        retain_history: true,
        protection: None,
    }
}

#[tokio::test]
async fn repeated_proxy_acknowledgements_do_not_write_duplicate_audit_rows() {
    let (control, path) = setup().await;
    control
        .seed_snapshot(ServiceSnapshot {
            revision: 1,
            services: vec![service()],
        })
        .await
        .unwrap();
    for _ in 0..3 {
        control.mark_services(1, None).await.unwrap();
        control.mark_rules(1, None).await.unwrap();
        control.mark_applied(1, None).await.unwrap();
    }
    let count = |control: &Control| {
        let db = control.db.clone();
        async move {
            db.run(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM audit WHERE action='control.apply'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(count(&control).await, 2);
    for _ in 0..3 {
        control
            .mark_services(1, Some("upstream unavailable".into()))
            .await
            .unwrap();
    }
    assert_eq!(count(&control).await, 3);
    control
        .mark_services(0, Some("stale error".into()))
        .await
        .unwrap();
    control.mark_rules(2, None).await.unwrap();
    assert_eq!(count(&control).await, 3);
    assert_eq!(
        control.desired().await.unwrap().apply_error.as_deref(),
        Some("upstream unavailable")
    );
    control.mark_services(1, None).await.unwrap();
    assert_eq!(count(&control).await, 4);
    assert!(control.desired().await.unwrap().apply_error.is_none());
    drop(control);
    std::fs::remove_dir_all(path).unwrap();
}
fn rule() -> Rule {
    Rule {
        id: "rule".into(),
        service_id: "service".into(),
        additional_service_ids: vec![],
        name: "test".into(),
        description: String::new(),
        enabled: true,
        mode: Mode::Shadow,
        priority: 1,
        expression: Expression::Predicate {
            field: "http.path".into(),
            op: Comparison::Eq,
            value: FieldValue::String("/secret".into()),
        },
        action: Action::Block { http_status: 403 },
        backend: "userspace".into(),
        expires_at_ms: None,
    }
}
#[tokio::test]
async fn colors_are_persistent_audited_and_independent_of_proxy_revisions() {
    let (c, path) = setup().await;
    c.put_service(actor("alice"), 0, service(), true, "create".into())
        .await
        .unwrap();
    c.set_service_color(
        actor("bob"),
        "service".into(),
        Some("#AB12CD".into()),
        "color".into(),
    )
    .await
    .unwrap();
    assert_eq!(c.service_colors().await.unwrap()["service"], "#ab12cd");
    assert_eq!(c.desired().await.unwrap().services.revision, 1);
    let reopened = Control::open(c.auth.clone()).await.unwrap();
    assert_eq!(
        reopened.service_colors().await.unwrap()["service"],
        "#ab12cd"
    );
    for invalid in ["red", "#123", "#12345g", "url(example)"] {
        assert!(
            c.set_service_color(
                actor("bob"),
                "service".into(),
                Some(invalid.into()),
                "invalid".into()
            )
            .await
            .is_err()
        );
    }
    assert!(
        c.set_service_color(
            actor("bob"),
            "missing".into(),
            Some("#123456".into()),
            "missing".into()
        )
        .await
        .is_err()
    );
    let audited =
        c.db.run(|db| {
            Ok(db.query_row(
                "SELECT actor_id FROM audit WHERE action='service.color'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(audited, "bob");
    c.set_service_color(actor("bob"), "service".into(), None, "reset".into())
        .await
        .unwrap();
    assert!(c.service_colors().await.unwrap().is_empty());
    c.set_service_color(
        actor("bob"),
        "service".into(),
        Some("#123456".into()),
        "color".into(),
    )
    .await
    .unwrap();
    c.delete(
        actor("alice"),
        1,
        "service",
        "service".into(),
        "delete".into(),
    )
    .await
    .unwrap();
    assert!(c.service_colors().await.unwrap().is_empty());
    drop(reopened);
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn revision_conflicts_atomic_reference_validation_and_real_ack() {
    let (c, path) = setup().await;
    c.put_service(actor("alice"), 0, service(), true, "test".into())
        .await
        .unwrap();
    let error = c
        .put_service(actor("bob"), 0, service(), false, "test".into())
        .await
        .err()
        .unwrap();
    assert!(error.downcast_ref::<Conflict>().is_some());
    c.put_rule(actor("bob"), 1, rule(), true, "test".into())
        .await
        .unwrap();
    assert!(
        c.delete(
            actor("alice"),
            2,
            "service",
            "service".into(),
            "test".into()
        )
        .await
        .is_err()
    );
    assert_eq!(c.desired().await.unwrap().services.revision, 2);
    c.mark_services(2, None).await.unwrap();
    let d = c.desired().await.unwrap();
    assert_eq!(d.services_applied_revision, 2);
    assert_eq!(d.rules_applied_revision, 0);
    assert_eq!(d.applied_revision, 0);
    c.mark_rules(2, None).await.unwrap();
    assert_eq!(c.desired().await.unwrap().applied_revision, 2);
    let records = c.attribution().await.unwrap();
    assert_eq!(
        records.iter().find(|r| r.id == "rule").unwrap().created_by,
        "bob"
    );
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn enforce_create_edit_and_bulk_enable_do_not_require_rehearsal() {
    let (c, path) = setup().await;
    c.put_service(actor("alice"), 0, service(), true, "test".into())
        .await
        .unwrap();
    let mut enforce = rule();
    enforce.mode = Mode::Enforce;
    c.put_rule(actor("alice"), 1, enforce.clone(), true, "test".into())
        .await
        .unwrap();
    enforce.priority = 2;
    c.put_rule(actor("bob"), 2, enforce.clone(), false, "test".into())
        .await
        .unwrap();
    enforce.enabled = false;
    c.put_rule(actor("alice"), 3, enforce, false, "test".into())
        .await
        .unwrap();
    c.bulk_rule_state(actor("bob"), 4, vec!["rule".into()], true, "test".into())
        .await
        .unwrap();
    let desired = c.desired().await.unwrap();
    assert_eq!(desired.rules.revision, 5);
    assert!(desired.rules.rules[0].enabled);
    assert_eq!(desired.rules.rules[0].mode, Mode::Enforce);
    let tests: i64 =
        c.db.run(|db| Ok(db.query_row("SELECT count(*) FROM rule_tests", [], |r| r.get(0))?))
            .await
            .unwrap();
    assert_eq!(tests, 0);
    let fields = std::collections::BTreeMap::from([(
        "http.path".into(),
        FieldValue::String("/secret".into()),
    )]);
    assert_eq!(
        c.test_rule(
            actor("alice"),
            "rule".into(),
            "service".into(),
            fields,
            "test".into()
        )
        .await
        .unwrap()["matched"],
        true
    );
    assert_eq!(
        c.desired().await.unwrap().rules.rules[0].mode,
        Mode::Enforce
    );
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn router_constructs_and_rejects_unauthenticated() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (c, path) = setup().await;
    let app = super::router(c.clone());
    let response = app
        .oneshot(
            Request::get("/api/v1/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn http_authorship_and_conflict_are_server_enforced() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    let (c, path) = setup().await;
    let id = crate::modules::users::create(
        &c.db,
        &c.auth.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let tokens = c
        .auth
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    let app = super::router(c.clone());
    let payload = serde_json::json!({"expected_revision":0,"transport":"tcp","listen_address":"127.0.0.1:21001","upstream_address":"127.0.0.1:21002","protocol_id":"http","enabled":true});
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/services")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", tokens.response.access_token),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["services"]["revision"], 1);
    assert_eq!(body["id"], body["services"]["services"][0]["id"]);
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/services")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", tokens.response.access_token),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let mut forged = payload;
    forged["created_by"] = "mallory".into();
    let response = app
        .oneshot(
            Request::post("/api/v1/services")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", tokens.response.access_token),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(forged.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(c.attribution().await.unwrap()[0].created_by, id);
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn cumulative_counters_are_durable_idempotent_and_epoch_aware() {
    let (control, path) = setup().await;
    control
        .put_service(actor("alice"), 0, service(), true, "test".into())
        .await
        .unwrap();
    control
        .put_rule(actor("alice"), 1, rule(), true, "test".into())
        .await
        .unwrap();
    for (epoch, n) in [("a", 2), ("a", 2), ("a", 5), ("b", 3)] {
        control
            .observe_counters(
                epoch.into(),
                std::collections::HashMap::from([("rule".into(), n)]),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        control
            .attribution()
            .await
            .unwrap()
            .iter()
            .find(|r| r.kind == "filter")
            .unwrap()
            .hit_count,
        8
    );
    drop(control);
    let db = Database::open(path.join("db")).await.unwrap();
    let auth = AuthState::new(
        db,
        Crypto::load(&path.join("key")).unwrap(),
        "https://hydra.test".into(),
    );
    let control = Control::open(auth).await.unwrap();
    control
        .observe_counters(
            "b".into(),
            std::collections::HashMap::from([("rule".into(), 3)]),
        )
        .await
        .unwrap();
    assert_eq!(
        control
            .attribution()
            .await
            .unwrap()
            .iter()
            .find(|r| r.kind == "filter")
            .unwrap()
            .hit_count,
        8
    );
    drop(control);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn maximum_bulk_is_atomic_audited_and_revision_checked() {
    let (c, path) = setup().await;
    c.put_service(actor("alice"), 0, service(), true, "service".into())
        .await
        .unwrap();
    let mut ids = Vec::new();
    let selected_id = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    for index in 0..256 {
        let mut r = rule();
        r.id = if index == 255 {
            selected_id.into()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        r.name = selected_id.into();
        ids.push(r.id.clone());
        c.put_rule(actor("alice"), index + 1, r, true, "create".into())
            .await
            .unwrap();
    }
    let result = c
        .bulk_rule_state(actor("bob"), 257, ids.clone(), false, "maximum-bulk".into())
        .await
        .unwrap();
    assert_eq!(result.rules.revision, 258);
    assert!(result.rules.rules.iter().all(|r| !r.enabled));
    // An exact detail lookup must search beyond a page of textual ID matches.
    crate::modules::users::create(
        &c.db,
        &c.auth.crypto,
        "reader".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let token = c
        .auth
        .login(
            "reader".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    use tower::ServiceExt;
    let response = crate::modules::filters::router(c.clone())
        .oneshot(
            axum::http::Request::get(format!("/api/v1/filters/{selected_id}"))
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", token.response.access_token),
                )
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(
        c.bulk_rule_state(actor("bob"), 257, ids.clone(), true, "stale".into())
            .await
            .is_err()
    );
    ids[255] = "missing".into();
    assert!(
        c.bulk_rule_state(actor("bob"), 258, ids, true, "invalid-selection".into())
            .await
            .is_err()
    );
    assert_eq!(c.desired().await.unwrap().rules.revision, 258);
    assert!(
        c.desired()
            .await
            .unwrap()
            .rules
            .rules
            .iter()
            .all(|r| !r.enabled)
    );
    c.db.run(|db|{let n:i64=db.query_row("SELECT count(*) FROM audit WHERE action='filter.state' AND actor_id='bob' AND request_id='maximum-bulk'",[],|r|r.get(0))?;assert_eq!(n,256);Ok(())}).await.unwrap();
    drop(c);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn forwarding_resolves_only_enabled_catalog_teams_and_rejects_bad_formats() {
    let (control, path) = setup().await;
    let farm = crate::modules::farm::Farm::open(
        control.auth.clone(),
        path.join("uploads"),
        std::sync::Arc::new(crate::modules::farm::UnavailableRunner),
    )
    .await
    .unwrap();
    control.db.run(|db|{db.execute("INSERT INTO farm_teams(id,name,host,enabled,revision,created_at,updated_at,author_id) VALUES('two','Team two','127.0.0.3',1,1,0,0,'owner')",[])?;Ok(())}).await.unwrap();
    let input = |pattern: &str| super::forwarding::ForwardingInput {
        team_id: "two".into(),
        target_port: 9000,
        flag_pattern: pattern.into(),
        framing: hydra_domain::ForwardFraming::Http1,
    };
    assert_eq!(
        input("FLAG[A-Z0-9]{8}")
            .resolve(&control)
            .await
            .unwrap()
            .target_address,
        "127.0.0.3:9000".parse().unwrap()
    );
    assert!(input("FLAG.*").resolve(&control).await.is_err());
    control
        .db
        .run(|db| {
            db.execute("UPDATE farm_teams SET enabled=0 WHERE id='two'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(input("FLAG[A-Z0-9]{8}").resolve(&control).await.is_err());
    drop(farm);
    drop(control);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn service_name_is_atomic_persistent_searchable_and_server_audited() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (control, path) = setup().await;
    let user = crate::modules::users::create(
        &control.db,
        &control.auth.crypto,
        "named-operator".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let token = control
        .auth
        .login(
            "named-operator".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap()
        .response
        .access_token;
    let app = super::router(control.clone());
    let request = |method: &str, url: &str, body: serde_json::Value| {
        Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let mut input = serde_json::json!({"name":"  Docs frontend  ","expected_revision":0,"transport":"tcp","listen_address":"127.0.0.1:21001","upstream_address":"127.0.0.1:21002","protocol_id":"http","enabled":true});
    let response = app
        .clone()
        .oneshot(request("POST", "/api/v1/services", input.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let id = body["id"].as_str().unwrap().to_owned();
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/services?q=Docs",
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    let page: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(page["items"][0]["name"], "Docs frontend");
    input["name"] = "Renamed docs".into();
    input["expected_revision"] = 0.into();
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PATCH",
                &format!("/api/v1/services/{id}"),
                input.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    input["expected_revision"] = 1.into();
    input["name"] = "\n\t".into();
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PATCH",
                &format!("/api/v1/services/{id}"),
                input.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(control.desired().await.unwrap().services.revision, 1);
    input["name"] = "Renamed docs".into();
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PATCH",
                &format!("/api/v1/services/{id}"),
                input.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    input["expected_revision"] = 2.into();
    input.as_object_mut().unwrap().remove("name");
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PATCH",
                &format!("/api/v1/services/{id}"),
                input.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    let reopened = Control::open(control.auth.clone()).await.unwrap();
    assert_eq!(reopened.service_names().await.unwrap()[&id], "Renamed docs");
    assert!(
        serde_json::to_value(&reopened.desired().await.unwrap().services.services[0])
            .unwrap()
            .get("name")
            .is_none()
    );
    control
        .db
        .run(move |db| {
            assert_eq!(
                db.query_row(
                    "SELECT name FROM service_names WHERE service_id=?1",
                    [&id],
                    |r| r.get::<_, String>(0)
                )?,
                "Renamed docs"
            );
            assert_eq!(
                db.query_row(
                    "SELECT actor_id FROM audit WHERE action='service.name' AND target=?1",
                    [id],
                    |r| r.get::<_, String>(0)
                )?,
                user
            );
            Ok(())
        })
        .await
        .unwrap();
    drop(reopened);
    drop(app);
    drop(control);
    std::fs::remove_dir_all(path).unwrap();
}
