mod client;
mod decoder;
mod encoder;
mod message;
mod name;
mod resolv;
mod rtype;

pub use self::client::DnsClient;
pub use self::message::DnsRecord;
pub use self::name::DomainName;
pub use self::resolv::system_nameserver;
pub use self::rtype::DnsType;
