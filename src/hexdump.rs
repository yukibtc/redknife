use std::fmt;

/// xxd-style rows preserve every octet
pub(super) struct HexDump<'a> {
    bytes: &'a [u8],
}

impl<'a> HexDump<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl fmt::Display for HexDump<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bytes.is_empty() {
            return writeln!(formatter, "    <empty>");
        }

        for (row_index, row) in self.bytes.chunks(16).enumerate() {
            let offset: usize = row_index * 16;
            write!(formatter, "    {offset:08x}: ")?;

            for group_index in 0..8 {
                for column in 0..2 {
                    match row.get(group_index * 2 + column) {
                        Some(byte) => write!(formatter, "{byte:02x}")?,
                        None => formatter.write_str("  ")?,
                    }
                }
                formatter.write_str(" ")?;
            }

            formatter.write_str(" ")?;

            for byte in row {
                let character: char = if (0x20..=0x7e).contains(byte) {
                    char::from(*byte)
                } else {
                    '.'
                };
                write!(formatter, "{character}")?;
            }

            writeln!(formatter)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert_eq!(HexDump::new(b"").to_string(), "    <empty>\n");
    }

    #[test]
    fn full_and_partial_rows_keep_offsets_and_columns_aligned() {
        let output: String = HexDump::new(b"0123456789ABCDEFG").to_string();
        let expected: String = format!(
            "    00000000: 3031 3233 3435 3637 3839 4142 4344 4546  0123456789ABCDEF\n\
             {indent}00000010: {hex:40} G\n",
            indent = "    ",
            hex = "47",
        );

        assert_eq!(output, expected);
    }

    #[test]
    fn binary_and_control_bytes_are_preserved_in_hex_only() {
        let output: String = HexDump::new(b"\0\x1b\n\xff A~").to_string();
        let expected: String = format!("    00000000: {:40} .... A~\n", "001b 0aff 2041 7e");

        assert_eq!(output, expected);
    }
}
