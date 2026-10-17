use crate::{
    infrastructure::db::Database,
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
mod creation;
pub use creation::{CreateError, CreateUser, create, create_authenticated, create_prompt};

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
        creation::validate_username(name)?;
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
