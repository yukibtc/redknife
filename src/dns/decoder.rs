use super::message::DnsMessageError;

pub(super) struct DnsDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> DnsDecoder<'a> {
    #[inline]
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    #[inline]
    pub(super) fn offset(&self) -> usize {
        self.offset
    }

    #[inline]
    pub(super) fn seek(&mut self, offset: usize) {
        self.offset = offset;
    }

    pub(super) fn read_u8(&mut self) -> Result<u8, DnsMessageError> {
        let value: u8 = self.peek_u8_at(self.offset)?;
        self.offset += 1;
        Ok(value)
    }

    pub(super) fn read_u16(&mut self) -> Result<u16, DnsMessageError> {
        let bytes: [u8; 2] = self.read_bytes(2)?.try_into().expect("two bytes");
        Ok(u16::from_be_bytes(bytes))
    }

    pub(super) fn read_u32(&mut self) -> Result<u32, DnsMessageError> {
        let bytes: [u8; 4] = self.read_bytes(4)?.try_into().expect("four bytes");
        Ok(u32::from_be_bytes(bytes))
    }

    pub(super) fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], DnsMessageError> {
        let start: usize = self.offset;
        let stop: usize = start
            .checked_add(length)
            .ok_or(DnsMessageError::UnexpectedEof)?;
        let bytes: &'a [u8] = self.slice_at(start, stop)?;
        self.offset = stop;
        Ok(bytes)
    }

    pub(super) fn skip(&mut self, length: usize) -> Result<(), DnsMessageError> {
        let stop: usize = self
            .offset
            .checked_add(length)
            .ok_or(DnsMessageError::UnexpectedEof)?;
        self.slice_at(self.offset, stop)?;
        self.offset = stop;
        Ok(())
    }

    #[inline]
    pub(super) fn peek_u8_at(&self, offset: usize) -> Result<u8, DnsMessageError> {
        self.bytes
            .get(offset)
            .copied()
            .ok_or(DnsMessageError::UnexpectedEof)
    }

    #[inline]
    pub(super) fn slice_at(&self, start: usize, stop: usize) -> Result<&'a [u8], DnsMessageError> {
        self.bytes
            .get(start..stop)
            .ok_or(DnsMessageError::UnexpectedEof)
    }
}
