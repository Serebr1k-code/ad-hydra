use std::{net::SocketAddr, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
/// Server-side endpoints only. Credentials never enter a worker environment.
#[derive(Clone)]
pub enum Checker {
    Https {
        address: SocketAddr,
        host: String,
        path: String,
        bearer: Option<String>,
        ca_certificate_der: Vec<u8>,
    },
    Http {
        address: SocketAddr,
        host: String,
        path: String,
        bearer: Option<String>,
    },
    CustomTcp {
        address: SocketAddr,
        request_template: String,
        accepted_prefix: String,
        secret: Option<String>,
        batch_size: usize,
    },
    Tcp {
        address: SocketAddr,
        secret: Option<String>,
    },
}
impl Checker {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Https {
                host,
                path,
                bearer,
                ca_certificate_der,
                ..
            } => {
                anyhow::ensure!(
                    !ca_certificate_der.is_empty() && ca_certificate_der.len() <= 65536,
                    "invalid checker CA"
                );
                let mut roots = rustls::RootCertStore::empty();
                roots.add(rustls::pki_types::CertificateDer::from(
                    ca_certificate_der.clone(),
                ))?;
                rustls::pki_types::ServerName::try_from(host.clone())?;
                Self::Http {
                    address: "127.0.0.1:1".parse()?,
                    host: host.clone(),
                    path: path.clone(),
                    bearer: bearer.clone(),
                }
                .validate()?;
            }
            Self::Http {
                host, path, bearer, ..
            } => {
                anyhow::ensure!(
                    host.len() <= 255
                        && !host.contains(['\r', '\n'])
                        && path.starts_with('/')
                        && path.len() <= 1024
                        && !path.contains(['\r', '\n', ' ']),
                    "invalid checker configuration"
                );
                if let Some(s) = bearer {
                    anyhow::ensure!(
                        s.len() <= 4096 && !s.contains(['\r', '\n']),
                        "invalid credential"
                    );
                }
            }
            Self::CustomTcp {
                request_template,
                accepted_prefix,
                secret,
                batch_size,
                ..
            } => {
                anyhow::ensure!(
                    (1..=32).contains(batch_size)
                        && request_template.len() <= 16384
                        && request_template.contains("{items}")
                        && !accepted_prefix.is_empty()
                        && accepted_prefix.len() <= 128,
                    "invalid custom checker"
                );
                anyhow::ensure!(
                    secret
                        .as_ref()
                        .is_none_or(|s| s.len() <= 4096 && !s.contains(['\r', '\n'])),
                    "invalid credential"
                );
            }
            Self::Tcp { secret, .. } => {
                if let Some(s) = secret {
                    anyhow::ensure!(
                        s.len() <= 4096 && !s.contains(['\r', '\n']),
                        "invalid credential"
                    );
                }
            }
        }
        Ok(())
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Https { .. } => "https",
            Self::Tcp { .. } => "tcp",
            Self::CustomTcp { .. } => "custom_tcp",
        }
    }
    pub fn batch_size(&self) -> usize {
        match self {
            Self::CustomTcp { batch_size, .. } => *batch_size,
            _ => 1,
        }
    }
    pub async fn send_batch(&self, items: &[(String, String)]) -> anyhow::Result<CheckerReply> {
        self.validate()?;
        anyhow::ensure!(
            !items.is_empty() && items.len() <= self.batch_size(),
            "invalid checker batch"
        );
        let Self::CustomTcp {
            address,
            request_template,
            accepted_prefix,
            secret,
            ..
        } = self
        else {
            return self.send_report(&items[0].1, &items[0].0).await;
        };
        let body = serde_json::to_string(
            &items
                .iter()
                .map(|(key, flag)| serde_json::json!({"flag":flag,"idempotency_key":key}))
                .collect::<Vec<_>>(),
        )?;
        let payload = request_template
            .replace("{items}", &body)
            .replace("{secret}", secret.as_deref().unwrap_or(""));
        anyhow::ensure!(payload.len() <= 65536, "checker request too large");
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = tokio::net::TcpStream::connect(address).await?;
            stream.write_all(payload.as_bytes()).await?;
            let mut reply = Vec::new();
            let mut byte = [0u8; 1];
            while reply.len() < 4096 {
                if stream.read(&mut byte).await? == 0 {
                    break;
                };
                reply.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&reply);
            let accepted = text.starts_with(accepted_prefix);
            let mut response = text.into_owned();
            for (_, flag) in items {
                response = response.replace(flag, "[redacted flag]")
            }
            if let Some(secret) = secret.as_ref().filter(|s| !s.is_empty()) {
                response = response.replace(secret, "[redacted credential]")
            };
            Ok::<_, anyhow::Error>(CheckerReply { accepted, response })
        })
        .await
        .map_err(|_| anyhow::anyhow!("checker timeout"))?
    }
    pub async fn send(&self, flag: &str, key: &str) -> anyhow::Result<bool> {
        Ok(self.send_report(flag, key).await?.accepted)
    }
    pub async fn send_report(&self, flag: &str, key: &str) -> anyhow::Result<CheckerReply> {
        tokio::time::timeout(Duration::from_secs(5),async {
            let address=match self {Self::Http{address,..}|Self::Https{address,..}|Self::Tcp{address,..}|Self::CustomTcp{address,..}=>address};
            let tcp=tokio::net::TcpStream::connect(address).await?;
            trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {} impl<T:tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin+Send> Io for T {}
            let mut stream:Box<dyn Io>=if let Self::Https{host,ca_certificate_der,..}=self {
              let mut roots=rustls::RootCertStore::empty();roots.add(rustls::pki_types::CertificateDer::from(ca_certificate_der.clone()))?;
              let provider=std::sync::Arc::new(rustls::crypto::ring::default_provider());
              let config=rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth();
              let connector=tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
              Box::new(connector.connect(rustls::pki_types::ServerName::try_from(host.clone())?,tcp).await?)
            }else{Box::new(tcp)};
            let payload=match self {
                Self::Http{host,path,bearer,..}|Self::Https{host,path,bearer,..}=>{let body=serde_json::to_string(&serde_json::json!({"flag":flag,"idempotency_key":key}))?;let auth=bearer.as_ref().map(|s|format!("Authorization: Bearer {s}\r\n")).unwrap_or_default();format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nIdempotency-Key: {key}\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())},
                Self::CustomTcp{request_template,secret,..}=>request_template.replace("{items}",&serde_json::to_string(&serde_json::json!([{"flag":flag,"idempotency_key":key}]))?).replace("{secret}",secret.as_deref().unwrap_or("")),
                Self::Tcp{secret,..}=>format!("{}\n",serde_json::to_string(&serde_json::json!({"flag":flag,"idempotency_key":key,"secret":secret}))?),
            };
            anyhow::ensure!(payload.len()<=65536,"checker request too large");
            stream.write_all(payload.as_bytes()).await?;
            let mut reply=Vec::new();let mut byte=[0;1];
            while reply.len()<4096 {
                if stream.read(&mut byte).await?==0 {break}
                reply.push(byte[0]);
                if matches!(self,Self::Http{..}|Self::Https{..}) && let Some(end)=reply.windows(4).position(|w|w==b"\r\n\r\n") {
                    let headers=String::from_utf8_lossy(&reply[..end]);
                    let length=headers.lines().filter_map(|l|l.split_once(':')).find(|(name,_)|name.eq_ignore_ascii_case("content-length")).and_then(|(_,n)|n.trim().parse::<usize>().ok());
                    if length.is_some_and(|n|n<=4096 && reply.len()>=end+4+n) {break}
                }
                if byte[0]==b'\n' && !matches!(self,Self::Http{..}|Self::Https{..}) {break}
            }
            let first=String::from_utf8_lossy(&reply);
            let accepted=match self {Self::Http{..}|Self::Https{..}=>first.lines().next().and_then(|line|{let mut parts=line.split_whitespace();let version=parts.next()?;if version!="HTTP/1.1" && version!="HTTP/1.0" {return None};parts.next()?.parse::<u16>().ok()}).is_some_and(|code|(200..300).contains(&code)),Self::Tcp{..}=>first.trim()=="OK",Self::CustomTcp{accepted_prefix,..}=>first.starts_with(accepted_prefix)};
            let secret=match self {Self::Http{bearer,..}|Self::Https{bearer,..}=>bearer, Self::Tcp{secret,..}|Self::CustomTcp{secret,..}=>secret};
            let mut response=first.replace(flag,"[redacted flag]");
            if let Some(secret)=secret.as_ref().filter(|s|!s.is_empty()){response=response.replace(secret,"[redacted credential]");}
            // Preserve response body and status, omit all HTTP headers (may contain credentials).
            if matches!(self,Self::Http{..}|Self::Https{..}) {let status=response.lines().next().unwrap_or("").to_owned();let body=response.split_once("\r\n\r\n").map(|(_,b)|b).unwrap_or("");response=format!("{status}\n{body}");}
            Ok::<_,anyhow::Error>(CheckerReply{accepted,response})
        }).await.map_err(|_|anyhow::anyhow!("checker timeout"))?
    }
}

pub struct CheckerReply {
    pub accepted: bool,
    pub response: String,
}
/// Extension boundary accepts only operator-configured endpoints; adapters never spawn programs.
pub trait CheckerAdapter {
    fn submit_batch<'a>(
        &'a self,
        items: &'a [(String, String)],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<CheckerReply>> + Send + 'a>,
    >;

    fn submit<'a>(
        &'a self,
        flag: &'a str,
        key: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<CheckerReply>> + Send + 'a>,
    >;
}
impl CheckerAdapter for Checker {
    fn submit_batch<'a>(
        &'a self,
        items: &'a [(String, String)],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<CheckerReply>> + Send + 'a>,
    > {
        Box::pin(self.send_batch(items))
    }

    fn submit<'a>(
        &'a self,
        flag: &'a str,
        key: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<CheckerReply>> + Send + 'a>,
    > {
        Box::pin(self.send_report(flag, key))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn http_passes_stable_key_and_accepts_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = vec![0; 8192];
            let n = socket.read(&mut data).await.unwrap();
            let text = String::from_utf8_lossy(&data[..n]);
            assert!(text.contains("Idempotency-Key: durable-id\r\n"));
            assert!(text.contains("Authorization: Bearer test-secret\r\n"));
            assert!(text.contains("\"flag\":\"FLAG123\""));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let adapter = Checker::Http {
            address,
            host: "localhost".into(),
            path: "/submit".into(),
            bearer: Some("test-secret".into()),
        };
        assert!(adapter.send("FLAG123", "durable-id").await.unwrap());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn tcp_uses_json_line_and_rejects_non_ok() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = vec![0; 8192];
            let n = socket.read(&mut data).await.unwrap();
            let value: serde_json::Value = serde_json::from_slice(&data[..n]).unwrap();
            assert_eq!(value["idempotency_key"], "same");
            socket.write_all(b"REJECTED\n").await.unwrap();
        });
        assert!(
            !Checker::Tcp {
                address,
                secret: None
            }
            .send("FLAG123", "same")
            .await
            .unwrap()
        );
        server.await.unwrap();
    }
    #[test]
    fn rejects_header_injection() {
        assert!(
            Checker::Http {
                address: "127.0.0.1:80".parse().unwrap(),
                host: "x\r\nInjected: yes".into(),
                path: "/".into(),
                bearer: None
            }
            .validate()
            .is_err()
        );
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[tokio::test]
    async fn custom_batch_preserves_keys_and_redacts_reply() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&buf[..n]).unwrap();
            assert_eq!(body[0]["idempotency_key"], "stable1");
            assert_eq!(body[1]["idempotency_key"], "stable2");
            stream
                .write_all(b"OK FLAG1 FLAG2 password\n")
                .await
                .unwrap();
        });
        let c = Checker::CustomTcp {
            address,
            request_template: "{items}\n".into(),
            accepted_prefix: "OK".into(),
            secret: Some("password".into()),
            batch_size: 2,
        };
        let reply = c
            .send_batch(&[
                ("stable1".into(), "FLAG1".into()),
                ("stable2".into(), "FLAG2".into()),
            ])
            .await
            .unwrap();
        assert!(reply.accepted);
        assert!(!reply.response.contains("FLAG1"));
        assert!(!reply.response.contains("password"));
        server.await.unwrap();
    }
}

#[cfg(test)]
mod tls_tests {
    use super::*;
    struct Certificates(std::path::PathBuf);
    impl Drop for Certificates {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn openssl(dir: &std::path::Path, args: &[&str]) {
        let result = std::process::Command::new("openssl")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("trusted openssl available");
        assert!(
            result.status.success(),
            "local test certificate generation failed"
        );
    }
    #[tokio::test]
    async fn https_verifies_pinned_ca_and_hostname_and_redacts_evidence() {
        use std::os::unix::fs::PermissionsExt;
        let certs = Certificates(
            std::env::temp_dir().join(format!("hydra-checker-tls-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&certs.0).unwrap();
        std::fs::set_permissions(&certs.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        openssl(
            &certs.0,
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "ca.key",
                "-out",
                "ca.pem",
                "-subj",
                "/CN=Hydra local test CA",
                "-days",
                "1",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
            ],
        );
        openssl(
            &certs.0,
            &[
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-subj",
                "/CN=localhost",
            ],
        );
        std::fs::write(certs.0.join("server.ext"),"subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature,keyEncipherment\n").unwrap();
        openssl(
            &certs.0,
            &[
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                "server.pem",
                "-days",
                "1",
                "-extfile",
                "server.ext",
            ],
        );
        openssl(
            &certs.0,
            &["x509", "-in", "ca.pem", "-outform", "DER", "-out", "ca.der"],
        );
        openssl(
            &certs.0,
            &[
                "x509",
                "-in",
                "server.pem",
                "-outform",
                "DER",
                "-out",
                "server.der",
            ],
        );
        openssl(
            &certs.0,
            &[
                "pkcs8",
                "-topk8",
                "-nocrypt",
                "-in",
                "server.key",
                "-outform",
                "DER",
                "-out",
                "server-key.der",
            ],
        );
        let ca = std::fs::read(certs.0.join("ca.der")).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                std::fs::read(certs.0.join("server.der")).unwrap(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                std::fs::read(certs.0.join("server-key.der")).unwrap(),
            )),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
            let mut requests = 0;
            for _ in 0..2 {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                if let Ok(mut stream) = acceptor.accept(stream).await {
                    let mut request = vec![0; 4096];
                    if let Ok(Ok(n)) =
                        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut request))
                            .await
                    {
                        if n == 0 {
                            continue;
                        }
                        // Never print the request: it deliberately contains test credentials.
                        let text = String::from_utf8_lossy(&request[..n]);
                        assert!(text.contains("Authorization: Bearer local-test-secret\r\n"));
                        assert!(text.contains("Idempotency-Key: tls-stable-key\r\n"));
                        requests += 1;
                        let body = "accepted FLAG123 local-test-secret";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Sensitive: local-test-secret\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                }
            }
            requests
        });
        let checker = Checker::Https {
            address,
            host: "localhost".into(),
            path: "/submit".into(),
            bearer: Some("local-test-secret".into()),
            ca_certificate_der: ca.clone(),
        };
        checker.validate().unwrap();
        let reply = checker
            .send_report("FLAG123", "tls-stable-key")
            .await
            .unwrap();
        assert!(reply.accepted);
        assert!(reply.response.contains("accepted"));
        assert!(!reply.response.contains("FLAG123"));
        assert!(!reply.response.contains("local-test-secret"));
        assert!(!reply.response.contains("X-Sensitive"));
        let wrong = Checker::Https {
            address,
            host: "wrong.localhost".into(),
            path: "/submit".into(),
            bearer: Some("local-test-secret".into()),
            ca_certificate_der: ca,
        };
        assert!(
            wrong
                .send_report("FLAG123", "tls-stable-key")
                .await
                .is_err()
        );
        assert_eq!(
            server.await.unwrap(),
            1,
            "wrong hostname must send no application request"
        );
        let path = certs.0.clone();
        drop(certs);
        assert!(!path.exists());
    }
}
