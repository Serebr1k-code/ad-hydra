use hydra_core::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::{
        auth::{AuthState, now},
        users,
    },
};
async fn setup() -> (AuthState, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("hydra-identity-{}", uuid::Uuid::new_v4()));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let db = Database::open(path.join("db")).await.unwrap();
    let crypto = Crypto::load(&path.join("key")).unwrap();
    (
        AuthState::new(db, crypto, "https://hydra.test".into()),
        path,
    )
}
#[tokio::test]
async fn rotation_reuse_revokes_access_and_absolute_deadline() {
    let (state, path) = setup().await;
    let id = users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let first = state
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    let claims = state
        .decode_at(&first.response.access_token, "access", now())
        .unwrap();
    assert_eq!(claims.exp - claims.iat, 60);
    assert!(
        state
            .decode_at(&first.response.access_token, "access", claims.exp)
            .is_err()
    );
    assert_eq!(first.deadline - claims.iat, 36000);
    assert_eq!(
        state
            .authenticate(&first.response.access_token)
            .await
            .unwrap()
            .id,
        id
    );
    let second = state
        .refresh(first.refresh.clone(), "test".into())
        .await
        .unwrap();
    assert_eq!(first.deadline, second.deadline);
    assert!(state.refresh(first.refresh, "test".into()).await.is_err());
    assert!(
        state
            .authenticate(&second.response.access_token)
            .await
            .is_err()
    );
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn deactivation_revokes_and_audit_is_append_only() {
    let (state, path) = setup().await;
    let id = users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let login = state
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    let actor = state
        .authenticate(&login.response.access_token)
        .await
        .unwrap();
    users::deactivate(&state.db, actor, id, "test".into())
        .await
        .unwrap();
    assert!(
        state
            .authenticate(&login.response.access_token)
            .await
            .is_err()
    );
    assert!(
        state
            .db
            .run(|db| {
                db.execute("DELETE FROM audit", [])?;
                Ok(())
            })
            .await
            .is_err()
    );
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn migration_checksum_mismatch_fails() {
    let (state, path) = setup().await;
    assert!(state.db.migrate(1, "SELECT 1;").await.is_err());
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn equal_accounts_manage_each_other_and_no_registration() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (state, path) = setup().await;
    let alice = users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let bob = users::create(
        &state.db,
        &state.crypto,
        "bob".into(),
        "another safe password".into(),
    )
    .await
    .unwrap();
    let a = state
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    let b = state
        .login("bob".into(), "another safe password".into(), "test".into())
        .await
        .unwrap();
    let aa = state.authenticate(&a.response.access_token).await.unwrap();
    let ba = state.authenticate(&b.response.access_token).await.unwrap();
    assert_eq!(
        users::list(&state.db, aa.clone(), "test".into())
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        users::list(&state.db, ba.clone(), "test".into())
            .await
            .unwrap()
            .len(),
        2
    );
    users::deactivate(&state.db, ba, alice, "test".into())
        .await
        .unwrap();
    assert!(state.authenticate(&a.response.access_token).await.is_err());
    // A fresh CLI account remains possible even after another full-rights account deactivates it.
    assert_ne!(
        bob,
        users::create(
            &state.db,
            &state.crypto,
            "recovery".into(),
            "recovery safe password".into()
        )
        .await
        .unwrap()
    );
    let app = hydra_core::modules::auth::router(state.clone());
    let response = app
        .oneshot(Request::post("/api/v1/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn cookies_and_origin_csrf_are_enforced() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    let (state, path) = setup().await;
    users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let app = hydra_core::modules::auth::router(state.clone());
    let login = r#"{"username":"alice","password":"correct horse battery"}"#;
    let denied = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header(header::ORIGIN, "https://hydra.test")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|h| h.to_str().unwrap().to_owned())
        .collect();
    assert!(cookies[0].contains("HttpOnly; Secure; SameSite=Strict; Path=/api/v1/auth"));
    assert!(cookies[1].contains("Path=/;"));
    let json: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let csrf = json["csrf_token"].as_str().unwrap();
    let cookie = cookies
        .iter()
        .map(|s| s.split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ");
    let denied = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/refresh")
                .header(header::ORIGIN, "https://hydra.test")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let ok = app
        .oneshot(
            Request::post("/api/v1/auth/refresh")
                .header(header::ORIGIN, "https://hydra.test")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

async fn account(state: &AuthState) -> String {
    users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap()
}
async fn login(state: &AuthState) -> hydra_core::modules::auth::Tokens {
    state
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_rotation_and_logout_never_reactivate_session() {
    let (state, path) = setup().await;
    account(&state).await;
    let first = login(&state).await;
    let (a, b) = tokio::join!(
        state.refresh(first.refresh.clone(), "a".into()),
        state.refresh(first.refresh, "b".into())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let winner = a.or(b).unwrap();
    assert!(
        state
            .authenticate(&winner.response.access_token)
            .await
            .is_err()
    );
    let first = login(&state).await;
    let (rotation, logout) = tokio::join!(
        state.refresh(first.refresh.clone(), "rotate".into()),
        state.logout(first.refresh, "logout".into())
    );
    logout.unwrap();
    assert!(
        state
            .authenticate(&first.response.access_token)
            .await
            .is_err()
    );
    if let Ok(rotation) = rotation {
        assert!(
            state
                .authenticate(&rotation.response.access_token)
                .await
                .is_err()
        );
        assert!(
            state
                .refresh(rotation.refresh, "again".into())
                .await
                .is_err()
        );
    }
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn expired_history_is_pruned_before_session_and_refresh_quotas() {
    let (state, path) = setup().await;
    let id = account(&state).await;
    let user = id.clone();
    state
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            let mut insert = tx.prepare("INSERT INTO sessions VALUES(?1,?2,0,0)")?;
            for n in 0..10000 {
                insert.execute(rusqlite::params![format!("expired-{n}"), user])?;
            }
            drop(insert);
            tx.execute(
                "INSERT INTO refresh_tokens VALUES('expired-token','expired-0',1)",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    let first = login(&state).await;
    state
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            assert_eq!(
                tx.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))?,
                1
            );
            tx.execute("INSERT INTO sessions VALUES('expired',?1,0,0)", [id])?;
            let mut insert = tx.prepare("INSERT INTO refresh_tokens VALUES(?1,'expired',1)")?;
            for n in 0..100000 {
                insert.execute([format!("expired-{n}")])?;
            }
            drop(insert);
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    let second = state
        .refresh(first.refresh.clone(), "rotate".into())
        .await
        .unwrap();
    state
        .db
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM refresh_tokens", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM refresh_tokens WHERE consumed=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    assert!(state.refresh(first.refresh, "reuse".into()).await.is_err());
    assert!(
        state
            .authenticate(&second.response.access_token)
            .await
            .is_err()
    );
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn database_and_sidecars_are_private_and_unsafe_paths_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (state, path) = setup().await;
    for name in ["db", "db-wal", "db-shm"] {
        assert_eq!(
            std::fs::metadata(path.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    symlink(path.join("db"), path.join("alias")).unwrap();
    assert!(Database::open(path.join("alias")).await.is_err());
    std::fs::write(path.join("unsafe"), b"").unwrap();
    std::fs::set_permissions(path.join("unsafe"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Database::open(path.join("unsafe")).await.is_err());
    symlink(path.join("db"), path.join("new-wal")).unwrap();
    assert!(Database::open(path.join("new")).await.is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Database::open(path.join("other")).await.is_err());
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn logout_clears_cookie_paths_used_by_login() {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    let (state, path) = setup().await;
    account(&state).await;
    let tokens = login(&state).await;
    let response = hydra_core::modules::auth::router(state.clone())
        .oneshot(
            Request::post("/api/v1/auth/logout")
                .header(header::ORIGIN, "https://hydra.test")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", tokens.response.access_token),
                )
                .header(
                    header::COOKIE,
                    format!(
                        "hydra_refresh={}; hydra_csrf={}",
                        tokens.refresh, tokens.response.csrf_token
                    ),
                )
                .header("x-csrf-token", tokens.response.csrf_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|h| h.to_str().unwrap())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("hydra_refresh=") && c.contains("Path=/api/v1/auth; Max-Age=0"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("hydra_csrf=") && c.contains("Path=/; Max-Age=0"))
    );
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn http2_split_cookie_headers_support_refresh() {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    let (state, path) = setup().await;
    users::create(
        &state.db,
        &state.crypto,
        "alice".into(),
        "correct horse battery".into(),
    )
    .await
    .unwrap();
    let tokens = state
        .login(
            "alice".into(),
            "correct horse battery".into(),
            "test".into(),
        )
        .await
        .unwrap();
    let app = hydra_core::modules::auth::router(state.clone());
    let request = Request::post("/api/v1/auth/refresh")
        .header(header::ORIGIN, "https://hydra.test")
        .header(
            header::COOKIE,
            format!("hydra_csrf={}", tokens.response.csrf_token),
        )
        .header(header::COOKIE, format!("hydra_refresh={}", tokens.refresh))
        .header("x-csrf-token", tokens.response.csrf_token)
        .body(Body::empty())
        .unwrap();
    assert_eq!(request.headers().get_all(header::COOKIE).iter().count(), 2);
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn account_update_checks_revision_and_revokes_sessions_without_exposing_passwords() {
    use hydra_core::modules::audit::Actor;
    let (state, path) = setup().await;
    let id = users::create(
        &state.db,
        &state.crypto,
        "managed".into(),
        "initial-password-123".into(),
    )
    .await
    .unwrap();
    let token = state
        .login(
            "managed".into(),
            "initial-password-123".into(),
            "login".into(),
        )
        .await
        .unwrap();
    users::update(
        &state,
        Actor::user(id.clone()),
        id.clone(),
        users::UpdateUser {
            expected_revision: 1,
            username: Some("renamed".into()),
            active: None,
            password: Some("replacement-password-456".into()),
        },
        "manage".into(),
    )
    .await
    .unwrap();
    assert!(
        state
            .authenticate(&token.response.access_token)
            .await
            .is_err()
    );
    assert!(
        users::update(
            &state,
            Actor::user(id.clone()),
            id.clone(),
            users::UpdateUser {
                expected_revision: 1,
                username: None,
                active: Some(false),
                password: None
            },
            "stale".into()
        )
        .await
        .is_err()
    );
    let token = state
        .login(
            "renamed".into(),
            "replacement-password-456".into(),
            "login".into(),
        )
        .await
        .unwrap();
    users::update(
        &state,
        Actor::user(id.clone()),
        id.clone(),
        users::UpdateUser {
            expected_revision: 2,
            username: None,
            active: Some(false),
            password: None,
        },
        "disable".into(),
    )
    .await
    .unwrap();
    assert!(
        state
            .authenticate(&token.response.access_token)
            .await
            .is_err()
    );
    users::update(
        &state,
        Actor::user("other-equal-user".into()),
        id,
        users::UpdateUser {
            expected_revision: 3,
            username: None,
            active: Some(true),
            password: None,
        },
        "enable".into(),
    )
    .await
    .unwrap();
    assert!(
        state
            .login(
                "renamed".into(),
                "replacement-password-456".into(),
                "login".into()
            )
            .await
            .is_ok()
    );
    state
        .db
        .run(|db| {
            let data: String = db.query_row(
                "SELECT group_concat(metadata) FROM audit WHERE action='user.update'",
                [],
                |r| r.get(0),
            )?;
            assert!(!data.contains("replacement-password"));
            assert!(data.contains("password_changed"));
            Ok(())
        })
        .await
        .unwrap();
    drop(state);
    std::fs::remove_dir_all(path).unwrap();
}
