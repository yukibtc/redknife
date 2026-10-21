use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum DnsTypeError {
    #[error("unknown DNS record type: {0}")]
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsType {
    A,
    Ns,
    Cname,
    Soa,
    Ptr,
    Mx,
    Txt,
    Aaaa,
    Opt,
    Unknown(u16),
}

impl DnsType {
    #[inline]
    pub fn from_u16(value: u16) -> Self {
        match value {
            1 => Self::A,
            2 => Self::Ns,
            5 => Self::Cname,
            6 => Self::Soa,
            12 => Self::Ptr,
            15 => Self::Mx,
            16 => Self::Txt,
            28 => Self::Aaaa,
            41 => Self::Opt,
            other => Self::Unknown(other),
        }
    }

    #[inline]
    pub fn to_u16(self) -> u16 {
        match self {
            Self::A => 1,
            Self::Ns => 2,
            Self::Cname => 5,
            Self::Soa => 6,
            Self::Ptr => 12,
            Self::Mx => 15,
            Self::Txt => 16,
            Self::Aaaa => 28,
            Self::Opt => 41,
            Self::Unknown(value) => value,
        }
    }
}

impl fmt::Display for DnsType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::A => write!(f, "A"),
            Self::Ns => write!(f, "NS"),
            Self::Cname => write!(f, "CNAME"),
            Self::Soa => write!(f, "SOA"),
            Self::Ptr => write!(f, "PTR"),
            Self::Mx => write!(f, "MX"),
            Self::Txt => write!(f, "TXT"),
            Self::Aaaa => write!(f, "AAAA"),
            Self::Opt => write!(f, "OPT"),
            Self::Unknown(value) => write!(f, "TYPE{value}"),
        }
    }
}

impl FromStr for DnsType {
    type Err = DnsTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value: &str = &value.to_ascii_uppercase();

        match value {
            "A" => return Ok(Self::A),
            "NS" => return Ok(Self::Ns),
            "CNAME" => return Ok(Self::Cname),
            "SOA" => return Ok(Self::Soa),
            "PTR" => return Ok(Self::Ptr),
            "MX" => return Ok(Self::Mx),
            "TXT" => return Ok(Self::Txt),
            "AAAA" => return Ok(Self::Aaaa),
            "OPT" => return Ok(Self::Opt),
            _ => {}
        }

        if let Some(value) = value.strip_prefix("TYPE") {
            if let Ok(value) = value.parse() {
                return Ok(Self::Unknown(value));
            }
        }

        Err(DnsTypeError::Unknown(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_values() {
        assert_eq!(DnsType::A.to_u16(), 1);
        assert_eq!(DnsType::Aaaa.to_u16(), 28);
        assert_eq!(DnsType::Opt.to_u16(), 41);
        assert_eq!(DnsType::from_u16(15), DnsType::Mx);
        assert_eq!(DnsType::from_u16(41), DnsType::Opt);
        assert_eq!(DnsType::from_u16(999), DnsType::Unknown(999));
    }

    #[test]
    fn test_parse() {
        assert_eq!("a".parse::<DnsType>().unwrap(), DnsType::A);
        assert_eq!("aaaa".parse::<DnsType>().unwrap(), DnsType::Aaaa);
        assert_eq!("cname".parse::<DnsType>().unwrap(), DnsType::Cname);
        assert_eq!("opt".parse::<DnsType>().unwrap(), DnsType::Opt);
        assert_eq!("TYPE42".parse::<DnsType>().unwrap(), DnsType::Unknown(42));
        assert!("foo".parse::<DnsType>().is_err());
        assert_eq!(DnsType::Txt.to_string(), "TXT");
        assert_eq!(DnsType::Opt.to_string(), "OPT");
    }
}
