use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::audit::{self, Actor},
};
use rusqlite::params;
use serde::Serialize;
use utoipa::ToSchema;
#[derive(Serialize, ToSchema)]
pub struct User {
    pub id: String,
    pub username: String,
    pub active: bool,
    pub created_at: String,
    pub revision: i64,
    pub updated_at: String,
}
pub async fn create(
    db: &Database,
    crypto: &Crypto,
    username: String,
    password: String,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !username.is_empty()
            && username.len() <= 64
            && username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_- .".contains(&b)),
        "invalid username"
    );
    anyhow::ensure!(password.len() >= 12, "password must have at least 12 bytes");
    let hash = crypto.hash(password).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let result = id.clone();
    db.run(move |c| {
        let tx = c.transaction()?;
        let count: i64 = tx.query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
        anyhow::ensure!(count < 1024, "user quota exhausted");
        let now = super::auth::now();
        tx.execute(
            "INSERT INTO users(id,username,password_hash,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",
            params![id, username, hash, now],
        )?;
        audit::append_change(&tx,&Actor::local_cli(),"user.create",&id,"cli",true,now,&serde_json::json!({"before":null,"after":{"username":username,"active":true},"source":"local_cli"}))?;
        tx.commit()?;
        Ok(result)
    })
    .await
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
pub async fn list(db: &Database, actor: Actor, request_id: String) -> anyhow::Result<Vec<User>> {
    db.run(move |c| {
        let tx = c.transaction()?;
        let users = tx
            .prepare(
                "SELECT id,username,active,strftime('%Y-%m-%dT%H:%M:%SZ',created_at,'unixepoch'),revision,strftime('%Y-%m-%dT%H:%M:%SZ',updated_at,'unixepoch') FROM users ORDER BY username LIMIT 1024",
            )?
            .query_map([], |r| {
                Ok(User {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    active: r.get(2)?,
                    created_at: r.get(3)?,
                    revision:r.get(4)?,
                    updated_at:r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        audit::append(
            &tx,
            &actor,
            "users.list",
            "users",
            &request_id,
            true,
            super::auth::now(),
        )?;
        tx.commit()?;
        Ok(users)
    })
    .await
}
pub async fn deactivate(
    db: &Database,
    actor: Actor,
    id: String,
    request_id: String,
) -> anyhow::Result<()> {
    db.run(move |c| {
        let tx = c.transaction()?;
        let before:bool=tx.query_row("SELECT active FROM users WHERE id=?1",[&id],|r|r.get(0))?;
        anyhow::ensure!(
            tx.execute("UPDATE users SET active=0,revision=revision+1,updated_at=?2 WHERE id=?1", params![id,super::auth::now()])? == 1,
            "user not found"
        );
        tx.execute("UPDATE sessions SET revoked=1 WHERE user_id=?1", [&id])?;
        audit::append_change(&tx,&actor,"user.deactivate",&id,&request_id,true,super::auth::now(),&serde_json::json!({"before":{"active":before},"after":{"active":false},"source":"http"}))?;
        tx.commit()?;
        Ok(())
    })
    .await
}

#[derive(serde::Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    pub expected_revision: i64,
    pub username: Option<String>,
    pub active: Option<bool>,
    pub password: Option<String>,
}
pub async fn update(
    auth: &super::auth::AuthState,
    actor: Actor,
    id: String,
    input: UpdateUser,
    request: String,
) -> anyhow::Result<()> {
    anyhow::ensure!(input.expected_revision > 0, "invalid revision");
    if let Some(name) = &input.username {
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_- .".contains(&b)),
            "invalid username"
        );
    }
    let hash = if let Some(password) = input.password {
        anyhow::ensure!(
            password.len() >= 12 && password.len() <= 1024,
            "password must contain 12..1024 bytes"
        );
        Some(auth.crypto.hash(password).await?)
    } else {
        None
    };
    auth.db.run(move|db|{
        let tx=db.transaction()?;
        let (name,active,revision):(String,bool,i64)=tx.query_row("SELECT username,active,revision FROM users WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        anyhow::ensure!(revision==input.expected_revision,"revision conflict");
        let new_name=input.username.unwrap_or_else(||name.clone());let new_active=input.active.unwrap_or(active);let reset=hash.is_some();
        tx.execute("UPDATE users SET username=?2,active=?3,password_hash=COALESCE(?4,password_hash),revision=revision+1,updated_at=?5 WHERE id=?1",params![id,new_name,new_active,hash,super::auth::now()])?;
        if reset || !new_active {tx.execute("UPDATE sessions SET revoked=1 WHERE user_id=?1",[&id])?;}
        audit::append_change(&tx,&actor,"user.update",&id,&request,true,super::auth::now(),&serde_json::json!({"before":{"username":name,"active":active,"revision":revision},"after":{"username":new_name,"active":new_active,"revision":revision+1},"password_changed":reset,"source":"http"}))?;
        tx.commit()?;Ok(())
    }).await
}
