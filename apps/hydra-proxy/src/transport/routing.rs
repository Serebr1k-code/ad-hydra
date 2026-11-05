use hydra_domain::ServiceSnapshot;
use std::{collections::HashSet, net::IpAddr};
pub fn reject_local_loops(snapshot: &ServiceSnapshot) -> anyhow::Result<()> {
    let mut local = HashSet::from([
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    ]);
    for interface in nix::ifaddrs::getifaddrs()? {
        if let Some(address) = interface.address {
            if let Some(v4) = address.as_sockaddr_in() {
                local.insert(IpAddr::V4(v4.ip()));
            }
            if let Some(v6) = address.as_sockaddr_in6() {
                local.insert(IpAddr::V6(v6.ip()));
            }
        }
    }
    validate(snapshot, &local)
}
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}
fn validate(snapshot: &ServiceSnapshot, local: &HashSet<IpAddr>) -> anyhow::Result<()> {
    for service in snapshot.services.iter().filter(|s| s.enabled) {
        let target = service.effective_upstream();
        let target_ip = canonical(target.ip());
        anyhow::ensure!(
            !snapshot.services.iter().any(|listener| listener.enabled
                && listener.transport == service.transport
                && listener.listen_address.port() == target.port()
                && (canonical(listener.listen_address.ip()) == target_ip
                    || listener.listen_address.ip().is_unspecified()
                        && local.iter().any(|ip| canonical(*ip) == target_ip))),
            "service upstream/forwarding target points to a local Hydra listener"
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_team_ip_is_rejected_but_remote_same_port_is_allowed() {
        let mut snapshot:ServiceSnapshot=serde_json::from_value(serde_json::json!({"revision":1,"services":[{"id":"svc","transport":"tcp","listen_address":"0.0.0.0:9000","upstream_address":"127.0.0.1:19000","enabled":true,"forwarding":{"team_id":"self","target_address":"192.0.2.1:9000","flag_pattern":"FLAG[A-Z0-9]{8}","framing":{"type":"http1"}}}]})).unwrap();
        let addresses = HashSet::from(["192.0.2.1".parse().unwrap()]);
        assert!(validate(&snapshot, &addresses).is_err());
        snapshot.services[0]
            .forwarding
            .as_mut()
            .unwrap()
            .target_address = "192.0.2.3:9000".parse().unwrap();
        assert!(validate(&snapshot, &addresses).is_ok());
    }
}
