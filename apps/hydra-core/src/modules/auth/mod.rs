mod http;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::audit::{self, Actor},
};
pub use http::{AuthError, Authenticated, IdentityApi, router};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
#[derive(Clone)]
pub struct AuthState {
    pub db: Database,
    pub crypto: Crypto,
    pub origin: String,
    attempts: std::sync::Arc<std::sync::Mutex<(i64, u32)>>,
}
impl AuthState {
    pub fn new(db: Database, crypto: Crypto, origin: String) -> Self {
        Self {
            db,
            crypto,
            origin,
            attempts: Default::default(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct Claims {
    pub sub: String,
    pub sid: String,
    pub jti: String,
    pub kind: String,
    pub iat: i64,
    pub exp: i64,
    pub nbf: i64,
    pub iss: String,
    pub aud: String,
}
#[derive(Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: u32,
    pub csrf_token: String,
}
pub struct Tokens {
    pub response: TokenResponse,
    pub refresh: String,
    pub deadline: i64,
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
impl AuthState {
    fn issue(&self, user: String, sid: String, at: i64, deadline: i64) -> anyhow::Result<Tokens> {
        let claims = Claims {
            sub: user,
            sid,
            jti: uuid::Uuid::new_v4().to_string(),
            kind: "refresh".into(),
            iat: at,
            exp: deadline,
            nbf: at,
            iss: "hydra-core".into(),
            aud: "hydra-web".into(),
        };
        let refresh = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA),
            &claims,
            &self.crypto.encoding,
        )?;
        let access = Claims {
            kind: "access".into(),
            exp: at + 60,
            jti: uuid::Uuid::new_v4().to_string(),
            ..claims
        };
        Ok(Tokens {
            response: TokenResponse {
                access_token: jsonwebtoken::encode(
                    &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA),
                    &access,
                    &self.crypto.encoding,
                )?,
                expires_in: 60,
                csrf_token: uuid::Uuid::new_v4().to_string(),
            },
            refresh,
            deadline,
        })
    }
    pub fn decode_at(&self, token: &str, kind: &str, at: i64) -> anyhow::Result<Claims> {
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
        validation.validate_exp = false;
        validation.set_issuer(&["hydra-core"]);
        validation.set_audience(&["hydra-web"]);
        validation.leeway = 0;
        let c = jsonwebtoken::decode::<Claims>(token, &self.crypto.decoding, &validation)?.claims;
        anyhow::ensure!(
            c.kind == kind && c.exp > at && c.iat <= at && c.nbf <= at,
            "expired or invalid token"
        );
        Ok(c)
    }
    pub async fn authenticate(&self, token: &str) -> anyhow::Result<Actor> {
        let c = self.decode_at(token, "access", now())?;
        self.active(c.clone(), now()).await?;
        Ok(Actor::user(c.sub))
    }
    async fn active(&self, claims: Claims, at: i64) -> anyhow::Result<()> {
        self.db.run(move|c|{let active:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id=?1 AND s.user_id=?2 AND s.revoked=0 AND s.expires_at>?3 AND u.active=1)",params![claims.sid,claims.sub,at],|r|r.get(0))?;anyhow::ensure!(active,"session revoked");Ok(())}).await
    }
    pub async fn login(
        &self,
        username: String,
        password: String,
        request_id: String,
    ) -> anyhow::Result<Tokens> {
        {
            let mut attempts = self
                .attempts
                .lock()
                .map_err(|_| anyhow::anyhow!("rate limiter unavailable"))?;
            let at = now();
            if at - attempts.0 >= 60 {
                *attempts = (at, 0)
            }
            anyhow::ensure!(attempts.1 < 20, "login rate limit");
            attempts.1 += 1;
        }
        anyhow::ensure!(
            username.len() <= 64 && password.len() <= 1024,
            "invalid credentials"
        );
        let user = self
            .db
            .run(move |c| {
                Ok(c.query_row(
                    "SELECT id,password_hash FROM users WHERE username=?1 AND active=1",
                    [username],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?)
            })
            .await?;
        let (id, hash) = match user {
            Some(user) => user,
            None => {
                self.db
                    .run(move |db| {
                        let tx = db.transaction()?;
                        audit::append(
                            &tx,
                            &Actor {
                                kind: "system".into(),
                                id: "unknown_login".into(),
                            },
                            "auth.login",
                            "session",
                            &request_id,
                            false,
                            now(),
                        )?;
                        tx.commit()?;
                        Ok(())
                    })
                    .await?;
                anyhow::bail!("invalid credentials")
            }
        };
        if !self.crypto.verify(password, hash).await? {
            let actor = Actor::user(id);
            self.db
                .run(move |c| {
                    let tx = c.transaction()?;
                    audit::append(
                        &tx,
                        &actor,
                        "auth.login",
                        "session",
                        &request_id,
                        false,
                        now(),
                    )?;
                    tx.commit()?;
                    Ok(())
                })
                .await?;
            anyhow::bail!("invalid credentials")
        }
        let at = now();
        let sid = uuid::Uuid::new_v4().to_string();
        let tokens = self.issue(id.clone(), sid.clone(), at, at + 36000)?;
        let hash = digest(&tokens.refresh);
        self.db.run(move|c|{let tx=c.transaction()?;prune_expired(&tx,at)?;let n:i64=tx.query_row("SELECT count(*) FROM sessions",[],|r|r.get(0))?;anyhow::ensure!(n<10000,"session quota exhausted");tx.execute("INSERT INTO sessions(id,user_id,expires_at) SELECT ?1,id,?3 FROM users WHERE id=?2 AND active=1",params![sid,id,at+36000])?;tx.execute("INSERT INTO refresh_tokens VALUES(?1,?2,0)",params![hash,sid])?;audit::append(&tx,&Actor::user(id),"auth.login",&sid,&request_id,true,at)?;tx.commit()?;Ok(())}).await?;
        Ok(tokens)
    }
    pub async fn refresh(&self, token: String, request_id: String) -> anyhow::Result<Tokens> {
        let at = now();
        let c = self.decode_at(&token, "refresh", at)?;
        self.active(c.clone(), at).await?;
        let tokens = self.issue(c.sub.clone(), c.sid.clone(), at, c.exp)?;
        let old = digest(&token);
        let new = digest(&tokens.refresh);
        let valid=self.db.run(move|db|{let tx=db.transaction()?;prune_expired(&tx,at)?;let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id=?1 AND s.user_id=?2 AND s.revoked=0 AND s.expires_at>?3 AND u.active=1)",params![c.sid,c.sub,at],|r|r.get(0))?;anyhow::ensure!(active,"session revoked");let consumed=tx.query_row("SELECT consumed FROM refresh_tokens WHERE digest=?1 AND session_id=?2",params![old,c.sid],|r|r.get::<_,bool>(0)).optional()?;if consumed!=Some(false){tx.execute("UPDATE sessions SET revoked=1 WHERE id=?1",[&c.sid])?;audit::append(&tx,&Actor::user(c.sub),"auth.refresh_reuse",&c.sid,&request_id,false,at)?;tx.commit()?;return Ok(false)}let count:i64=tx.query_row("SELECT count(*) FROM refresh_tokens",[],|r|r.get(0))?;anyhow::ensure!(count<100000,"refresh quota exhausted");tx.execute("UPDATE refresh_tokens SET consumed=1 WHERE digest=?1",[old])?;tx.execute("INSERT INTO refresh_tokens VALUES(?1,?2,0)",params![new,c.sid])?;audit::append(&tx,&Actor::user(c.sub),"auth.refresh",&c.sid,&request_id,true,at)?;tx.commit()?;Ok(true)}).await?;
        anyhow::ensure!(valid, "refresh reuse detected");
        Ok(tokens)
    }
    pub async fn logout(&self, token: String, request_id: String) -> anyhow::Result<()> {
        let c = self.decode_at(&token, "refresh", now())?;
        self.db
            .run(move |db| {
                let tx = db.transaction()?;
                tx.execute("UPDATE sessions SET revoked=1 WHERE id=?1", [&c.sid])?;
                audit::append(
                    &tx,
                    &Actor::user(c.sub),
                    "auth.logout",
                    &c.sid,
                    &request_id,
                    true,
                    now(),
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
    }
}

// Retain all consumed token digests until the session's absolute deadline so
// replay detection survives rotation and restart. Expired history is disposable.
fn prune_expired(tx: &rusqlite::Transaction<'_>, at: i64) -> anyhow::Result<()> {
    tx.execute("DELETE FROM refresh_tokens WHERE session_id IN (SELECT id FROM sessions WHERE expires_at<=?1)", [at])?;
    tx.execute("DELETE FROM sessions WHERE expires_at<=?1", [at])?;
    Ok(())
}
