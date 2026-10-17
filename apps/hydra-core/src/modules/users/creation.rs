use super::User;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::audit::{self, Actor},
};
use rusqlite::params;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUser {
    pub username: String,
    /// Password length is measured in UTF-8 bytes and must be 12..1024.
    pub password: String,
}

#[derive(Debug)]
pub enum CreateError {
    InvalidUsername,
    InvalidPassword,
    Duplicate,
    Quota,
}
impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidUsername => "invalid username",
            Self::InvalidPassword => "password must contain 12..1024 bytes",
            Self::Duplicate => "username already exists",
            Self::Quota => "user quota exhausted",
        })
    }
}
impl std::error::Error for CreateError {}

pub(super) fn validate_username(username: &str) -> anyhow::Result<()> {
    if username.is_empty()
        || username.len() > 64
        || !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_- .".contains(&b))
    {
        return Err(CreateError::InvalidUsername.into());
    }
    Ok(())
}

pub async fn create(
    db: &Database,
    crypto: &Crypto,
    username: String,
    password: String,
) -> anyhow::Result<String> {
    Ok(create_account(
        db,
        crypto,
        CreateUser { username, password },
        Actor::local_cli(),
        "cli".into(),
        "local_cli",
    )
    .await?
    .id)
}

pub async fn create_authenticated(
    db: &Database,
    crypto: &Crypto,
    input: CreateUser,
    actor: Actor,
    request_id: String,
) -> anyhow::Result<User> {
    create_account(db, crypto, input, actor, request_id, "http").await
}

async fn create_account(
    db: &Database,
    crypto: &Crypto,
    input: CreateUser,
    actor: Actor,
    request_id: String,
    source: &'static str,
) -> anyhow::Result<User> {
    validate_username(&input.username)?;
    if !(12..=1024).contains(&input.password.len()) {
        return Err(CreateError::InvalidPassword.into());
    }
    let hash = crypto.hash(input.password).await?;
    let id = uuid::Uuid::new_v4().to_string();
    db.run(move |c| {
        let tx = c.transaction()?;
        let count: i64 = tx.query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
        if count >= 1024 {
            return Err(CreateError::Quota.into());
        }
        let now = crate::modules::auth::now();
        let username = input.username;
        if let Err(error) = tx.execute(
            "INSERT INTO users(id,username,password_hash,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",
            params![id, username, hash, now],
        ) {
            if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE) {
                return Err(CreateError::Duplicate.into());
            }
            return Err(error.into());
        }
        audit::append_change(&tx, &actor, "user.create", &id, &request_id, true, now,
            &serde_json::json!({"before":null,"after":{"username":username,"active":true},"source":source}))?;
        let user = tx.query_row(
            "SELECT id,username,active,strftime('%Y-%m-%dT%H:%M:%SZ',created_at,'unixepoch'),revision,strftime('%Y-%m-%dT%H:%M:%SZ',updated_at,'unixepoch') FROM users WHERE id=?1",
            [&id],
            |r| Ok(User { id:r.get(0)?, username:r.get(1)?, active:r.get(2)?, created_at:r.get(3)?, revision:r.get(4)?, updated_at:r.get(5)? }),
        )?;
        tx.commit()?;
        Ok(user)
    }).await
}

pub async fn create_prompt(
    db: &Database,
    crypto: &Crypto,
    username: String,
) -> anyhow::Result<String> {
    let password =
        tokio::task::spawn_blocking(|| rpassword::prompt_password("Password: ")).await??;
    create(db, crypto, username, password).await
}
