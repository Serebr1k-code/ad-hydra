use super::*;
use hydra_rewrite::{Format, Frame};
#[tokio::test]
async fn mappings_survive_connections_restart_and_remain_namespaced() {
    let dir = std::env::temp_dir().join(format!("hydra-forward-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let format = Format::compile("FLAG[A-Z0-9]{8}").unwrap();
    let original = b"prefix FLAG12345678 suffix".to_vec();
    let store = Store::open(dir.join("mappings")).await.unwrap();
    let mut request = Frame::payload(original.clone(), 0);
    store
        .rewrite("service1", &format, &mut request, false)
        .await
        .unwrap();
    assert_ne!(request.wire, original);
    let substituted = request.wire.clone();
    let mut repeated = Frame::payload(original.clone(), 0);
    store
        .rewrite("service1", &format, &mut repeated, false)
        .await
        .unwrap();
    assert_eq!(repeated.wire, substituted);
    drop(store);
    let store = Store::open(dir.join("mappings")).await.unwrap();
    let mut response = Frame::payload(substituted.clone(), 0);
    store
        .rewrite("service1", &format, &mut response, true)
        .await
        .unwrap();
    assert_eq!(response.wire, original);
    let mut other = Frame::payload(substituted.clone(), 0);
    store
        .rewrite("service2", &format, &mut other, true)
        .await
        .unwrap();
    assert_eq!(other.wire, substituted);
    let mut ambiguous = Frame::payload(substituted, 0);
    assert!(
        store
            .rewrite("service1", &format, &mut ambiguous, false)
            .await
            .is_err()
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn tcp_forwarding_substitutes_split_flags_and_restores_across_connections() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let dir = std::env::temp_dir().join(format!("hydra-forward-tcp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(dir.join("mappings")).await.unwrap();
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service:hydra_domain::Service=serde_json::from_value(serde_json::json!({"id":"mirror","transport":"tcp","listen_address":"127.0.0.1:31001","upstream_address":"127.0.0.1:31002","enabled":true,"forwarding":{"team_id":"two","target_address":upstream.local_addr().unwrap().to_string(),"flag_pattern":"FLAG[A-Z0-9]{8}","framing":{"type":"delimited","delimiter":"\n"}}})).unwrap();
    store.configure(std::slice::from_ref(&service)).unwrap();
    let target = tokio::spawn(async move {
        let (mut first, _) = upstream.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut b = [0; 1];
        while first.read(&mut b).await.unwrap() != 0 {
            bytes.push(b[0]);
            if b[0] == b'\n' {
                break;
            }
        }
        assert_ne!(bytes, b"write FLAG12345678\n");
        assert!(bytes.starts_with(b"write FLAG"));
        first.write_all(b"stored\n").await.unwrap();
        first.shutdown().await.unwrap();
        drop(first);
        let (mut second, _) = upstream.accept().await.unwrap();
        let mut get = [0; 4];
        second.read_exact(&mut get).await.unwrap();
        assert_eq!(&get, b"get\n");
        second.write_all(&bytes).await.unwrap();
        second.shutdown().await.unwrap();
    });
    let mut first = connect(&service, Some(store.clone())).await.unwrap();
    for byte in b"write FLAG12345678\n" {
        first.write_all(&[*byte]).await.unwrap();
        tokio::task::yield_now().await;
    }
    first.shutdown().await.unwrap();
    let mut response = Vec::new();
    first.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"stored\n");
    drop(first);
    drop(store);
    let restarted = Store::open(dir.join("mappings")).await.unwrap();
    restarted.configure(std::slice::from_ref(&service)).unwrap();
    let mut second = connect(&service, Some(restarted.clone())).await.unwrap();
    second.write_all(b"get\n").await.unwrap();
    second.shutdown().await.unwrap();
    response.clear();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        second.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response, b"write FLAG12345678\n");
    target.await.unwrap();
    drop(second);
    drop(restarted);
    std::fs::remove_dir_all(dir).unwrap();
}
