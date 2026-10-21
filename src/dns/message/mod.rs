use bytes::Bytes;
use rand::RngExt;

use crate::dns::DomainName;

mod error;
mod header;
mod question;
mod record;

pub use self::error::DnsMessageError;
pub use self::header::DnsHeader;
use self::header::{DnsFlags, DnsOpcode, DnsRCode};
pub use self::question::DnsQuestion;
pub use self::record::DnsRecord;
use super::decoder::DnsDecoder;
use super::encoder::DnsEncoder;
use super::rtype::DnsType;

const QUERY_CLASS_IN: u16 = 1;

pub struct DnsMessage {
    pub header: DnsHeader,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<DnsRecord>,
    pub authorities: Vec<DnsRecord>,
    pub additionals: Vec<DnsRecord>,
}

impl DnsMessage {
    pub(super) fn query(domain: DomainName, r#type: DnsType) -> Self {
        Self {
            header: DnsHeader {
                id: rand::rng().random(),
                flags: DnsFlags {
                    qr: false,
                    opcode: DnsOpcode::Query,
                    aa: false,
                    tc: false,
                    rd: true,
                    ra: false,
                    ad: false,
                    cd: false,
                    rcode: DnsRCode::NoError,
                },
                qdcount: 1,
                ancount: 0,
                nscount: 0,
                arcount: 0,
            },
            questions: vec![DnsQuestion {
                name: domain,
                qtype: r#type.to_u16(),
                qclass: QUERY_CLASS_IN,
            }],
            answers: Vec::new(),
            authorities: Vec::new(),
            additionals: Vec::new(),
        }
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DnsMessageError> {
        let mut decoder: DnsDecoder = DnsDecoder::new(bytes);

        let header: DnsHeader = DnsHeader::decode(decoder.read_bytes(12)?)?;

        if header.flags.tc {
            return Err(DnsMessageError::Truncated);
        }

        let mut questions: Vec<DnsQuestion> = Vec::with_capacity(header.qdcount as usize);
        for _ in 0..header.qdcount {
            questions.push(DnsQuestion::read(&mut decoder)?);
        }

        let mut answers: Vec<DnsRecord> = Vec::with_capacity(header.ancount as usize);
        for _ in 0..header.ancount {
            answers.push(DnsRecord::read(&mut decoder)?);
        }

        let mut authorities: Vec<DnsRecord> = Vec::with_capacity(header.nscount as usize);
        for _ in 0..header.nscount {
            authorities.push(DnsRecord::read(&mut decoder)?);
        }

        let mut additionals: Vec<DnsRecord> = Vec::with_capacity(header.arcount as usize);
        for _ in 0..header.arcount {
            additionals.push(DnsRecord::read(&mut decoder)?);
        }

        Ok(Self {
            header,
            questions,
            answers,
            authorities,
            additionals,
        })
    }

    pub(super) fn encode(&self) -> Bytes {
        let mut encoder: DnsEncoder = DnsEncoder::new();

        let header: DnsHeader = DnsHeader {
            qdcount: self.questions.len() as u16,
            ancount: self.answers.len() as u16,
            nscount: self.authorities.len() as u16,
            arcount: self.additionals.len() as u16,
            ..self.header
        };

        encoder.write_bytes(&header.encode());

        for question in &self.questions {
            question.write(&mut encoder);
        }

        for answer in &self.answers {
            answer.write(&mut encoder);
        }

        for authority in &self.authorities {
            authority.write(&mut encoder);
        }

        for additional in &self.additionals {
            additional.write(&mut encoder);
        }

        encoder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::record::DnsRecordData;
    use super::*;

    #[test]
    fn test_query_roundtrip() {
        let domain: DomainName = "example.com".parse().unwrap();
        let message: DnsMessage = DnsMessage::query(domain, DnsType::A);
        let bytes: Bytes = message.encode();
        let decoded: DnsMessage = DnsMessage::decode(&bytes).unwrap();

        assert_eq!(decoded.header.id, message.header.id);
        assert_eq!(decoded.header.qdcount, 1);
        assert_eq!(decoded.header.ancount, 0);
        assert!(!decoded.header.flags.qr);
        assert!(decoded.header.flags.rd);
        assert_eq!(decoded.questions.len(), 1);
        assert_eq!(decoded.questions[0].name, "example.com.".parse().unwrap());
        assert_eq!(decoded.questions[0].qtype, DnsType::A.to_u16());
        assert_eq!(decoded.questions[0].qclass, QUERY_CLASS_IN);
    }

    #[test]
    fn test_query_custom_type() {
        let domain: DomainName = "example.com".parse().unwrap();
        let message: DnsMessage = DnsMessage::query(domain, DnsType::Aaaa);
        let bytes: Bytes = message.encode();
        let decoded: DnsMessage = DnsMessage::decode(&bytes).unwrap();

        assert_eq!(decoded.questions[0].qtype, DnsType::Aaaa.to_u16());
    }

    #[test]
    fn test_response_roundtrip() {
        let mut message: DnsMessage = DnsMessage::query("example.com".parse().unwrap(), DnsType::A);
        message.header.flags.qr = true;
        message.answers.push(DnsRecord {
            name: "example.com".parse().unwrap(),
            r#type: DnsType::A,
            class: 1,
            ttl: 300,
            rdata: DnsRecordData::A("93.184.216.34".parse().unwrap()),
        });
        message.answers.push(DnsRecord {
            name: "www.example.com".parse().unwrap(),
            r#type: DnsType::Cname,
            class: 1,
            ttl: 300,
            rdata: DnsRecordData::Cname("example.com".parse().unwrap()),
        });
        message.answers.push(DnsRecord {
            name: "example.com".parse().unwrap(),
            r#type: DnsType::Txt,
            class: 1,
            ttl: 300,
            rdata: DnsRecordData::Txt(vec!["v=spf1 -all".to_string()]),
        });

        let bytes: Bytes = message.encode();
        let decoded: DnsMessage = DnsMessage::decode(&bytes).unwrap();

        assert!(decoded.header.flags.is_response());
        assert_eq!(decoded.answers, message.answers);
    }

    #[test]
    fn test_compressed_name() {
        let mut encoder: DnsEncoder = DnsEncoder::new();

        encoder.write_bytes(
            &DnsHeader {
                id: 42,
                flags: DnsFlags {
                    qr: true,
                    opcode: DnsOpcode::Query,
                    aa: false,
                    tc: false,
                    rd: true,
                    ra: true,
                    ad: false,
                    cd: false,
                    rcode: DnsRCode::NoError,
                },
                qdcount: 1,
                ancount: 1,
                nscount: 0,
                arcount: 0,
            }
            .encode(),
        );

        encoder.write_bytes(&[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ]);
        encoder.write_u16(1);
        encoder.write_u16(1);

        encoder.write_bytes(&[0xC0, 0x0C]);
        encoder.write_u16(1);
        encoder.write_u16(1);
        encoder.write_u32(300);
        encoder.write_u16(4);
        encoder.write_bytes(&[1, 2, 3, 4]);

        let decoded: DnsMessage = DnsMessage::decode(encoder.finish().as_ref()).unwrap();

        assert_eq!(decoded.header.id, 42);
        assert_eq!(decoded.answers.len(), 1);
        assert_eq!(decoded.answers[0].name, "example.com.".parse().unwrap());
        assert_eq!(
            decoded.answers[0].rdata,
            DnsRecordData::A("1.2.3.4".parse().unwrap())
        );
    }
}
