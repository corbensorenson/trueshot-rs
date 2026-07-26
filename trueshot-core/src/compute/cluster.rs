use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::time::Duration;

pub struct ClusterNode {
    mdns: ServiceDaemon,
}

impl ClusterNode {
    pub fn new(port: u16) -> anyhow::Result<Self> {
        let mdns = ServiceDaemon::new()?;
        let service_type = "_trueshot._tcp.local.";
        let instance_name = format!("trueshot-{}", hostname::get()?.to_string_lossy());
        let host_name = format!("{}.local.", hostname::get()?.to_string_lossy());
        let properties = [("version", "6.0.0")];

        let my_service = ServiceInfo::new(
            service_type,
            &instance_name,
            &host_name,
            "",
            port,
            &properties[..],
        )?
        .enable_addr_auto();

        mdns.register(my_service)?;
        Ok(Self { mdns })
    }

    pub fn discover_peers(&self) -> Vec<String> {
        let receiver = self
            .mdns
            .browse("_trueshot._tcp.local.")
            .expect("Failed to browse");
        let mut peers = Vec::new();

        let now = std::time::Instant::now();
        while now.elapsed() < Duration::from_secs(1) {
            if let Ok(event) = receiver.recv_timeout(Duration::from_millis(100)) {
                if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                    let addrs = info.get_addresses();
                    for addr in addrs {
                        peers.push(format!("{}:{}", addr, info.get_port()));
                    }
                }
            }
        }
        peers
    }
}
