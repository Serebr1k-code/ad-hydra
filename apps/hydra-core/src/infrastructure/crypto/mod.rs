use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{PasswordHash, SaltString},
};
use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use rand::rngs::OsRng;
use std::{path::Path, sync::Arc};
use tokio::sync::Semaphore;
#[derive(Clone)]
pub struct Crypto {
    pub encoding: Arc<jsonwebtoken::EncodingKey>,
    pub decoding: Arc<jsonwebtoken::DecodingKey>,
    hashing: Arc<Semaphore>,
}
impl Crypto {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let key = if path.exists() {
            let m = std::fs::symlink_metadata(path)?;
            anyhow::ensure!(
                m.is_file()
                    && m.permissions().mode() & 0o077 == 0
                    && m.uid() == std::fs::metadata("/proc/self")?.uid(),
                "JWT key must be a regular private file (0600)"
            );
            SigningKey::from_pkcs8_pem(&std::fs::read_to_string(path)?)?
        } else {
            let key = SigningKey::generate(&mut OsRng);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            use std::io::Write;
            f.write_all(key.to_pkcs8_pem(Default::default())?.as_bytes())?;
            f.sync_all()?;
            key
        };
        Ok(Self {
            encoding: Arc::new(jsonwebtoken::EncodingKey::from_ed_pem(
                key.to_pkcs8_pem(Default::default())?.as_bytes(),
            )?),
            decoding: Arc::new(jsonwebtoken::DecodingKey::from_ed_pem(
                key.verifying_key()
                    .to_public_key_pem(Default::default())?
                    .as_bytes(),
            )?),
            hashing: Arc::new(Semaphore::new(1)),
        })
    }
    pub async fn hash(&self, password: String) -> anyhow::Result<String> {
        self.password(password, None).await.map(|v| v.0)
    }
    pub async fn verify(&self, password: String, hash: String) -> anyhow::Result<bool> {
        self.password(password, Some(hash)).await.map(|v| v.1)
    }
    async fn password(
        &self,
        password: String,
        hash: Option<String>,
    ) -> anyhow::Result<(String, bool)> {
        anyhow::ensure!(password.len() <= 1024, "password too long");
        let permit = self
            .hashing
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("password hashing busy"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match hash {
                Some(hash) => Ok((
                    String::new(),
                    Argon2::default()
                        .verify_password(
                            password.as_bytes(),
                            &PasswordHash::new(&hash)
                                .map_err(|_| anyhow::anyhow!("invalid password hash"))?,
                        )
                        .is_ok(),
                )),
                None => Ok((
                    Argon2::default()
                        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
                        .map_err(|_| anyhow::anyhow!("password hashing failed"))?
                        .to_string(),
                    true,
                )),
            }
        })
        .await?
    }
}
