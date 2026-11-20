//! TLS termination and upstream TLS for TCP services.
//!
//! The Proxy presents an operator-supplied certificate to clients and/or speaks TLS
//! to the upstream. ALPN offers HTTP/1.1 only: the native HTTP decoder cannot parse
//! HTTP/2, so a backend that insists on h2 stays unparsed rather than mis-decoded.
use anyhow::{Context, ensure};
use hydra_domain::ServiceTls;
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
    server::WebPkiClientVerifier,
};
use std::{io::Cursor, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// A relay stream whose concrete type depends on whether TLS was negotiated.
pub(crate) trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}
pub(crate) type ClientStream = Box<dyn AsyncStream>;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Server configuration for `terminate` services, built once per listener.
pub fn server_config(tls: &ServiceTls) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    let mut chain = Cursor::new(
        tls.certificate_pem
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    let certificates: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut chain)
        .collect::<Result<_, _>>()
        .context("parse TLS certificate chain")?;
    ensure!(
        !certificates.is_empty(),
        "TLS certificate chain contains no certificate"
    );
    let mut key = Cursor::new(
        tls.private_key_pem
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut key)?.context("TLS private key is empty")?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(WebPkiClientVerifier::no_client_auth())
        .with_single_cert(certificates, key)
        .context("TLS certificate and private key do not match")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub fn acceptor(tls: &ServiceTls) -> anyhow::Result<TlsAcceptor> {
    Ok(TlsAcceptor::from(server_config(tls)?))
}

/// Upstream connector. `insecure` accepts self-signed competition backends.
pub fn connector(tls: &ServiceTls) -> anyhow::Result<TlsConnector> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?;
    let mut config = if tls.insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider())))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

/// The upstream name used for SNI and verification: explicit, else the upstream IP.
pub fn server_name(
    tls: &ServiceTls,
    address: std::net::SocketAddr,
) -> anyhow::Result<ServerName<'static>> {
    let name = tls
        .server_name
        .clone()
        .unwrap_or_else(|| address.ip().to_string());
    ServerName::try_from(name).map_err(|_| anyhow::anyhow!("invalid TLS server name"))
}

/// Handshake with the upstream. Kept here so the transport never grows TLS logic.
pub async fn connect(
    stream: TcpStream,
    tls: &ServiceTls,
    address: std::net::SocketAddr,
) -> anyhow::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    connector(tls)?
        .connect(server_name(tls, address)?, stream)
        .await
        .context("upstream TLS handshake failed")
}

#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);
impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
