//! `grimoire_link` as a library stub (CLI `grimoire-link`): `tcp` of `grimoire_debug` always on,
//! `sigilc` as a library.

pub fn config(addr: std::net::SocketAddrV4, token: [u8; 32]) -> grimoire_debug::TcpConfig {
    grimoire_debug::TcpConfig::new(addr, token)
}

pub fn unit_id(path: &str) -> Option<u64> {
    grimoire_sigilc::unit_id_for_canonical_path(path).map(|id| id.0)
}
