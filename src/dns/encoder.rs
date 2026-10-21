use bytes::{BufMut, Bytes, BytesMut};

pub(super) struct DnsEncoder {
    bytes: BytesMut,
}

impl DnsEncoder {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            bytes: BytesMut::new(),
        }
    }

    #[inline]
    pub(super) fn write_u8(&mut self, value: u8) {
        self.bytes.put_u8(value);
    }

    #[inline]
    pub(super) fn write_u16(&mut self, value: u16) {
        self.bytes.put_u16(value);
    }

    #[inline]
    pub(super) fn write_u32(&mut self, value: u32) {
        self.bytes.put_u32(value);
    }

    #[inline]
    pub(super) fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    #[inline]
    pub(super) fn finish(self) -> Bytes {
        self.bytes.freeze()
    }
}
