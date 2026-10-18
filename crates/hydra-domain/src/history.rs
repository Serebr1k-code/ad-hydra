//! Resource policy shared by Core configuration and Proxy history storage.
pub const MIN_PER_SERVICE_MIB: u32 = 1;
pub const MAX_PER_SERVICE_MIB: u32 = 1024;
pub const MIB: u64 = 1024 * 1024;
pub fn default_per_service_mib() -> u32 {
    128
}
