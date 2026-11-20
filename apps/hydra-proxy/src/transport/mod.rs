pub(crate) mod protection;
mod routing;
mod session;
mod tcp;
pub(crate) mod tls;
mod udp;
use crate::{config::Limits, telemetry::Telemetry};
use hydra_domain::{Service, ServiceSnapshot, Transport};
pub use session::{RelayBase, RelayTransport};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::watch,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

type Key = (Transport, SocketAddr);
enum Socket {
    Tcp(Arc<TcpListener>),
    Udp(Arc<UdpSocket>),
}
struct Listener {
    service: watch::Sender<Option<Service>>,
    task: tokio::task::JoinHandle<()>,
}
pub struct Manager {
    listeners: HashMap<Key, Listener>,
    limits: Limits,
    telemetry: Arc<Telemetry>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}
pub struct Prepared(Vec<(Key, Socket)>);
impl Manager {
    pub fn new(limits: Limits, telemetry: Arc<Telemetry>, shutdown: CancellationToken) -> Self {
        Self {
            listeners: HashMap::new(),
            limits,
            telemetry,
            shutdown,
            tasks: TaskTracker::new(),
        }
    }
    pub async fn prepare(&self, snapshot: &ServiceSnapshot) -> anyhow::Result<Prepared> {
        routing::reject_local_loops(snapshot)?;
        for service in &snapshot.services {
            if let Some(f) = &service.forwarding {
                hydra_rewrite::validate(f)?;
            }
        }
        let mut prepared = Vec::new();
        for s in snapshot.services.iter().filter(|s| s.enabled) {
            let key = (s.transport, s.listen_address);
            if self.listeners.contains_key(&key) {
                continue;
            }
            let socket = match s.transport {
                Transport::Tcp => Socket::Tcp(Arc::new(TcpListener::bind(s.listen_address).await?)),
                Transport::Udp => Socket::Udp(Arc::new(UdpSocket::bind(s.listen_address).await?)),
            };
            prepared.push((key, socket));
        }
        Ok(Prepared(prepared))
    }
    pub async fn activate(&mut self, snapshot: &ServiceSnapshot, prepared: Prepared) {
        self.telemetry.configure_protection(&snapshot.services);
        if let Some(store) = self
            .telemetry
            .forwarding
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            store
                .configure(&snapshot.services)
                .expect("validated forwarding formats");
        }
        let desired: HashMap<_, _> = snapshot
            .services
            .iter()
            .filter(|s| s.enabled)
            .map(|s| ((s.transport, s.listen_address), s))
            .collect();
        let removed: Vec<_> = self
            .listeners
            .keys()
            .filter(|k| !desired.contains_key(k))
            .copied()
            .collect();
        for key in removed {
            if let Some(listener) = self.listeners.remove(&key) {
                listener.service.send_replace(None);
                let _ = listener.task.await;
            }
        }
        for (key, listener) in &self.listeners {
            if let Some(service) = desired.get(key)
                && listener.service.borrow().as_ref() != Some(*service)
            {
                listener.service.send_replace(Some((*service).clone()));
            }
        }
        for (key, socket) in prepared.0 {
            let (sender, receiver) = watch::channel(Some(desired[&key].clone()));
            let task = match socket {
                Socket::Tcp(s) => tokio::spawn(tcp::listen(
                    s,
                    receiver,
                    self.limits.clone(),
                    self.telemetry.clone(),
                    self.shutdown.clone(),
                    self.tasks.clone(),
                )),
                Socket::Udp(s) => tokio::spawn(udp::listen(
                    s,
                    receiver,
                    self.limits.clone(),
                    self.telemetry.clone(),
                    self.shutdown.clone(),
                )),
            };
            self.listeners.insert(
                key,
                Listener {
                    service: sender,
                    task,
                },
            );
        }
    }
    pub async fn stop(&mut self) {
        self.shutdown.cancel();
        for (_, listener) in self.listeners.drain() {
            let _ = listener.task.await;
        }
        self.tasks.close();
        self.tasks.wait().await;
    }
}
