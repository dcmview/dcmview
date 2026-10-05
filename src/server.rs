mod api;
mod catalog;
pub mod lifecycle;
pub mod runtime;
mod tags;
#[cfg(unix)]
mod unix_socket;
mod web;

pub use api::{AccessToken, AppState};
pub use catalog::{FileRegistry, RegistryStatus};
pub use lifecycle::RequestActivity;
pub use runtime::{BoundServer, ServerConfig};

use axum::Router;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn router(state: AppState) -> Router {
    api::router(state)
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

pub(crate) fn is_non_loopback_bind(ip: std::net::IpAddr) -> bool {
    !ip.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::is_non_loopback_bind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn detects_loopback_and_non_loopback_bind_addresses() {
        assert!(!is_non_loopback_bind(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_non_loopback_bind(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_non_loopback_bind(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(is_non_loopback_bind(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }
}
