# Phase 2 identity integration

Export `infrastructure::{db,crypto}` and `modules::{auth,users,audit}`. No changes to existing bootstrap are included in this subtask.

Startup: `let db = Database::open(path).await?; let crypto = Crypto::load(key_path)?; let auth = AuthState::new(db.clone(),crypto.clone(),configured_https_origin);` Then merge `auth::router(auth.clone())` into public Router. Merge `auth::IdentityApi::openapi()` into generated OpenAPI. Keep docs behind an access-token authentication middleware using `auth.authenticate(bearer).await`.

CLI: `users::create(&db,&crypto,username,password).await?` or `users::create_prompt(&db,&crypto,username).await?`. CLI writes local UID attribution. At least 12 password bytes; explicitly warn argv users about process-list/shell-history disclosure. Creating a fresh account is recovery after deactivation. No HTTP account creation route.

DB: `Database::run(move |connection| -> anyhow::Result<T> {...}).await` runs serialized SQLite work on blocking pool; 32 outstanding slots, immediate overload error. `Database::migrate(version, include_str!(...)).await` adds transaction/checksum migrations. Audit append APIs accept caller transaction so mutation and audit commit atomically; `append_change` permits bounded redacted metadata. Audit quota fails mutations closed at 1M records; users cap1024, sessions10000, refresh token records100000. Retention/archive tooling still needed.

AuthState `authenticate(&str)->Result<Actor>` verifies EdDSA, exact expiration and active stored session. Access TTL60s, refresh deadline36000s absolute. Login Origin must match configured origin. Refresh/logout require matching Origin plus `X-CSRF-Token` equal `hydra_csrf` cookie; response returns CSRF token alongside memory-only access token. Cookies Secure, Strict, refresh HttpOnly, scoped `/api/v1/auth`; HTTPS required. Unknown credentials return generic401. Request IDs are generated server-side. All accounts identical rights.

Direct dependencies added to core manifest: rusqlite0.38/bundled, argon2.5, rand.8, sha2.10, jsonwebtoken9, ed25519-dalek2 with rand_core/pkcs8/pem, rpassword7. Existing serde/serde_json/tokio/uuid/utoipa/axum/anyhow used.

Limitations: global login rate limit20/minute; no per-address limit, no timed lockout; authenticated list is bounded but no cursor yet; password buffers are not explicitly zeroized; audit hard quota needs operational archival; deleted expired sessions are not automatically retained/pruned. Error mapping currently uses generic401 for dependency overload as well. No session-specific revoke endpoint yet; logout/deactivation revoke immediately. Audit event listing provided separately by parent if needed.
