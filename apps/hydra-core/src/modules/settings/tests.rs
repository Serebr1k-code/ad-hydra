use super::*;
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
struct Fixture {
    dir: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("hydra-settings-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let path = dir.join("operations.json");
        std::fs::write(&path,br#"{"flag_pattern":"FLAG_[A-Z]+","auto_submit_captured":false,"checker":{"kind":"tcp","address":"127.0.0.1:1234","secret":"private-test-credential"}}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self { dir, path }
    }
    fn store(&self) -> file::ConfigFile {
        file::ConfigFile::new(Some(self.path.clone())).unwrap()
    }
    fn editable(&self) -> (String, EditableConfig) {
        let saved = self.store().read().unwrap().unwrap();
        (
            saved.revision,
            EditableConfig::redacted(&saved.settings).unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn persist_reopen_revision_permissions_and_credentials() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let original = std::fs::metadata(&fixture.path).unwrap();
    let (revision, mut config) = fixture.editable();
    assert!(
        !serde_json::to_string(&config)
            .unwrap()
            .contains("private-test-credential")
    );
    config.flag_pattern = Some("CHANGED_[A-Z]+".into());
    let saved = store.save(&revision, &config).unwrap();
    assert_ne!(saved.revision, revision);
    let reopen = operations::Settings::load(Some(&fixture.path)).unwrap();
    assert_eq!(reopen.flag_pattern.as_deref(), Some("CHANGED_[A-Z]+"));
    assert!(
        matches!(reopen.checker,Some(operations::Checker::Tcp{secret:Some(ref s),..}) if s=="private-test-credential")
    );
    let metadata = std::fs::metadata(&fixture.path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(
        (metadata.uid(), metadata.gid()),
        (original.uid(), original.gid())
    );
    assert!(matches!(
        store.save(&revision, &config),
        Err(file::SaveError::Conflict)
    ));
    assert_ne!(store.running_revision, Some(saved.revision.clone()));
    assert_eq!(fixture.store().running_revision, Some(saved.revision));
}
#[test]
fn secret_replacement_clear_and_kind_switch_are_explicit() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let (revision, mut config) = fixture.editable();
    if let Some(CheckerConfig::Tcp { secret, .. }) = &mut config.checker {
        *secret = SecretUpdate::Replace {
            value: Some("replacement".into()),
        };
    }
    let saved = store.save(&revision, &config).unwrap();
    assert!(
        matches!(saved.settings.checker,Some(operations::Checker::Tcp{secret:Some(ref s),..}) if s=="replacement")
    );
    let mut config = EditableConfig::redacted(&saved.settings).unwrap();
    if let Some(CheckerConfig::Tcp { secret, .. }) = &mut config.checker {
        *secret = SecretUpdate::Replace { value: None };
    }
    let cleared = store.save(&saved.revision, &config).unwrap();
    assert!(matches!(
        cleared.settings.checker,
        Some(operations::Checker::Tcp { secret: None, .. })
    ));
    config.checker = Some(CheckerConfig::Http {
        address: "127.0.0.1:80".into(),
        host: "localhost".into(),
        path: "/".into(),
        bearer: SecretUpdate::Keep,
        credential_configured: true,
    });
    let changed = store.save(&cleared.revision, &config).unwrap();
    assert!(matches!(
        changed.settings.checker,
        Some(operations::Checker::Http { bearer: None, .. })
    ));
}
#[test]
fn invalid_updates_leave_actual_file_unchanged() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.path).unwrap();
    let (revision, mut config) = fixture.editable();
    config.flag_pattern = Some("(".into());
    assert!(matches!(
        fixture.store().save(&revision, &config),
        Err(file::SaveError::Invalid(_))
    ));
    config.flag_pattern = Some(".*".into());
    assert!(matches!(
        fixture.store().save(&revision, &config),
        Err(file::SaveError::Invalid(_))
    ));
    config.flag_pattern = None;
    config.execution_mode = operations::ExecutionMode::Host;
    assert!(matches!(
        fixture.store().save(&revision, &config),
        Err(file::SaveError::Invalid(_))
    ));
    config.host_runtime = Some(operations::HostRuntimeSettings {
        uv: "relative".into(),
        work_dir: "/tmp/work".into(),
    });
    assert!(matches!(
        fixture.store().save(&revision, &config),
        Err(file::SaveError::Invalid(_))
    ));
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    // Unknown fields are rejected rather than lost when saving a known DTO.
    let mut raw: serde_json::Value = serde_json::from_slice(&before).unwrap();
    raw["unknown_future_secret"] = serde_json::json!("must-not-drop");
    std::fs::write(&fixture.path, serde_json::to_vec(&raw).unwrap()).unwrap();
    assert!(fixture.store_checked().is_err());
}
impl Fixture {
    fn store_checked(&self) -> anyhow::Result<file::ConfigFile> {
        file::ConfigFile::new(Some(self.path.clone()))
    }
}
#[test]
fn bounded_private_file_and_no_config_readonly() {
    assert!(
        file::ConfigFile::new(None)
            .unwrap()
            .readonly_reason()
            .is_some()
    );
    let fixture = Fixture::new();
    std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(fixture.store_checked().is_err());
    std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&fixture.path, vec![b' '; 65537]).unwrap();
    assert!(fixture.store_checked().is_err());
    std::fs::remove_file(&fixture.path).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &fixture.path).unwrap();
    assert!(fixture.store_checked().is_err());
}
#[tokio::test]
async fn save_actor_audit_outcome_and_restart_view_are_real() {
    let fixture = Fixture::new();
    let auth = AuthState::new(
        crate::infrastructure::db::Database::open(fixture.dir.join("db"))
            .await
            .unwrap(),
        crate::infrastructure::crypto::Crypto::load(&fixture.dir.join("key")).unwrap(),
        "https://localhost".into(),
    );
    let running = operations::Settings::load(Some(&fixture.path)).unwrap();
    let state = Settings::new(
        auth,
        SettingsView::from_settings(&running),
        Some(fixture.path.clone()),
    )
    .unwrap();
    let view = state.current_view().await.unwrap();
    assert!(!view.restart_required);
    let mut config = view.editable_config;
    config.flag_pattern = Some("NEW_[A-Z]+".into());
    let original = std::fs::read(&fixture.path).unwrap();
    for headers in [
        HeaderMap::new(),
        HeaderMap::from_iter([(header::ORIGIN, "https://foreign.invalid".parse().unwrap())]),
    ] {
        let denied = update_settings(
            State(state.clone()),
            Authenticated(audit::Actor::user("operator-id".into())),
            headers,
            Json(UpdateSettingsRequest {
                saved_revision: view.saved_revision.clone().unwrap(),
                editable_config: config.clone(),
            }),
        )
        .await;
        assert!(matches!(denied, Err((StatusCode::FORBIDDEN, _))));
        assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
    }
    let saved = update_settings(
        State(state.clone()),
        Authenticated(audit::Actor::user("operator-id".into())),
        HeaderMap::from_iter([(header::ORIGIN, "https://localhost".parse().unwrap())]),
        Json(UpdateSettingsRequest {
            saved_revision: view.saved_revision.clone().unwrap(),
            editable_config: config.clone(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(saved.restart_required);
    assert_eq!(saved.flag_pattern.as_deref(), Some("FLAG_[A-Z]+"));
    assert_eq!(
        saved.editable_config.flag_pattern.as_deref(),
        Some("NEW_[A-Z]+")
    );
    let conflict = update_settings(
        State(state.clone()),
        Authenticated(audit::Actor::user("operator-id".into())),
        HeaderMap::from_iter([(header::ORIGIN, "https://localhost".parse().unwrap())]),
        Json(UpdateSettingsRequest {
            saved_revision: view.saved_revision.unwrap(),
            editable_config: config,
        }),
    )
    .await;
    assert!(matches!(conflict, Err((StatusCode::CONFLICT, _))));
    let events=state.auth.db.run(|db| {
        Ok(db.prepare("SELECT actor_id,success,metadata FROM audit WHERE action='settings.update.result' ORDER BY id")?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,bool>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?)
    }).await.unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|event| event.0 == "operator-id"));
    assert_eq!(
        events.iter().map(|event| event.1).collect::<Vec<_>>(),
        vec![false, false, true, false]
    );
    assert!(
        events
            .iter()
            .all(|event| !event.2.contains("private-test-credential"))
    );
    assert_eq!(saved.access_lifetime_seconds, 60);
    assert_eq!(saved.refresh_absolute_lifetime_seconds, 36000);
    assert_eq!(saved.worker_concurrency, 1);
}

#[test]
fn request_cannot_supply_actor_or_config_file_path() {
    let fixture = Fixture::new();
    let (revision, config) = fixture.editable();
    let mut request = serde_json::json!({"saved_revision":revision,"editable_config":config});
    request["actor"] = serde_json::json!({"id":"forged"});
    assert!(serde_json::from_value::<UpdateSettingsRequest>(request.clone()).is_err());
    request.as_object_mut().unwrap().remove("actor");
    request["path"] = serde_json::json!("/etc/another-file");
    assert!(serde_json::from_value::<UpdateSettingsRequest>(request).is_err());
}

#[test]
fn history_defaults_and_bounds_preserve_existing_configuration() {
    let fixture = Fixture::new();
    assert_eq!(
        operations::Settings::default().traffic_history_per_service_mib,
        128
    );
    assert_eq!(
        operations::Settings::default().traffic_history_compression_level,
        1
    );
    assert_eq!(
        operations::Settings::load(Some(&fixture.path))
            .unwrap()
            .traffic_history_per_service_mib,
        128
    );
    let store = fixture.store();
    let (revision, mut config) = fixture.editable();
    let before = std::fs::read(&fixture.path).unwrap();
    for invalid in [0, 1025] {
        config.traffic_history_per_service_mib = invalid;
        assert!(matches!(
            store.save(&revision, &config),
            Err(file::SaveError::Invalid(_))
        ));
        assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    }
    config.traffic_history_per_service_mib = 256;
    config.traffic_history_compression_level = 19;
    store.save(&revision, &config).unwrap();
    assert_eq!(
        operations::Settings::load(Some(&fixture.path))
            .unwrap()
            .traffic_history_per_service_mib,
        256
    );
    assert_eq!(
        operations::Settings::load(Some(&fixture.path))
            .unwrap()
            .traffic_history_compression_level,
        19
    );
    assert_eq!(
        store.running_restart_revision,
        Some(file::restart_revision(
            &operations::Settings::load(Some(&fixture.path)).unwrap()
        ))
    );
}

#[tokio::test]
async fn history_updates_are_live_audited_and_pending_until_proxy_acknowledges() {
    let fixture = Fixture::new();
    let auth = AuthState::new(
        crate::infrastructure::db::Database::open(fixture.dir.join("db"))
            .await
            .unwrap(),
        crate::infrastructure::crypto::Crypto::load(&fixture.dir.join("key")).unwrap(),
        "https://localhost".into(),
    );
    let running = operations::Settings::load(Some(&fixture.path)).unwrap();
    let (policy, mut receive) = tokio::sync::watch::channel((128 * 1024 * 1024u64, 1i32));
    let proxy = Arc::new(tokio::sync::RwLock::new(crate::api::ProxyView {
        connected: true,
        history_per_service_bytes: 128 * 1024 * 1024,
        history_compression_level: Some(1),
        ..Default::default()
    }));
    let state = Settings::new(
        auth,
        SettingsView::from_settings(&running),
        Some(fixture.path.clone()),
    )
    .unwrap()
    .with_history_policy(policy, proxy.clone());
    let view = state.current_view().await.unwrap();
    assert!(!view.history_apply_pending);
    let mut config = view.editable_config;
    config.traffic_history_per_service_mib = 256;
    config.traffic_history_compression_level = 7;
    let saved = update_settings(
        State(state.clone()),
        Authenticated(audit::Actor::user("operator-id".into())),
        HeaderMap::from_iter([(header::ORIGIN, "https://localhost".parse().unwrap())]),
        Json(UpdateSettingsRequest {
            saved_revision: view.saved_revision.unwrap(),
            editable_config: config,
        }),
    )
    .await
    .unwrap()
    .0;
    receive.changed().await.unwrap();
    assert_eq!(*receive.borrow(), (256 * 1024 * 1024, 7));
    assert!(!saved.restart_required);
    assert!(saved.history_apply_pending);
    assert_eq!(saved.traffic_history_per_service_mib_applied, Some(128));
    assert!(saved.compression_apply_pending);
    assert_eq!(saved.core_compression_level_applied, 7);
    assert_eq!(hydra_compression::level(), 7);
    proxy.write().await.history_per_service_bytes = 256 * 1024 * 1024;
    proxy.write().await.history_compression_level = Some(7);
    assert!(!state.current_view().await.unwrap().history_apply_pending);
    assert!(
        !state
            .current_view()
            .await
            .unwrap()
            .compression_apply_pending
    );
    proxy.write().await.connected = false;
    let disconnected = state.current_view().await.unwrap();
    assert!(disconnected.history_apply_pending);
    assert_eq!(disconnected.traffic_history_per_service_mib_applied, None);
    assert!(disconnected.compression_apply_pending);
    let count=state.auth.db.run(|db|Ok(db.query_row("SELECT count(*) FROM audit WHERE actor_id='operator-id' AND action='settings.update.result' AND success=1",[],|r|r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(count, 1);
}
