mod ioctl;
mod layer;
mod mac;
#[cfg(target_os = "linux")]
mod sysfs;

pub use self::ioctl::read_ipv4;
pub use self::layer::datalink::*;
pub use self::mac::{MacAddr, MacAddrParseError};
