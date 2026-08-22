//! A line splitter that reuses one buffer instead of allocating a `String`
//! per line. Every other module reads a JSONL file through this so the
//! CRLF/BOM/missing-final-newline handling only has to be right once.

use std::io::{self, BufRead};

/// Splits a `BufRead` into lines without the trailing newline.
///
/// Handles a leading UTF-8 BOM (stripped once, from the very first line
/// only), both `\n` and `\r\n` terminators, and a final line with no
/// terminator at all. Lines are returned as raw bytes because whether they
/// are valid UTF-8 is the caller's problem, not this one.
pub struct LineReader<R> {
    reader: R,
    buf: Vec<u8>,
    checked_bom: bool,
    line_number: usize,
}

impl<R: BufRead> LineReader<R> {
    pub fn new(reader: R) -> Self {
        LineReader {
            reader,
            buf: Vec::new(),
            checked_bom: false,
            line_number: 0,
        }
    }

    /// The 1-based number of the line most recently returned, or 0 if
    /// `next_line` has not been called yet.
    pub fn line_number(&self) -> usize {
        self.line_number
    }

    /// Reads the next line into the internal buffer and returns a borrow of
    /// it, or `None` at end of input. The borrow is only valid until the
    /// next call.
    pub fn next_line(&mut self) -> io::Result<Option<&[u8]>> {
        self.buf.clear();
        let read = self.reader.read_until(b'\n', &mut self.buf)?;
        if read == 0 {
            return Ok(None);
        }

        if !self.checked_bom {
            self.checked_bom = true;
            if self.buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.buf.drain(0..3);
            }
        }

        if self.buf.last() == Some(&b'\n') {
            self.buf.pop();
            if self.buf.last() == Some(&b'\r') {
                self.buf.pop();
            }
        }

        self.line_number += 1;
        Ok(Some(&self.buf))
    }
}

#[cfg(test)]
mod tests {
    use super::LineReader;
    use std::io::Cursor;

    #[test]
    fn splits_multiple_lines() {
        let mut r = LineReader::new(Cursor::new(&b"a\nb\nc\n"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"a"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"b"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"c"[..]));
        assert_eq!(r.next_line().unwrap(), None);
    }

    #[test]
    fn keeps_last_line_without_trailing_newline() {
        let mut r = LineReader::new(Cursor::new(&b"a\nb"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"a"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"b"[..]));
        assert_eq!(r.next_line().unwrap(), None);
    }

    #[test]
    fn strips_crlf() {
        let mut r = LineReader::new(Cursor::new(&b"a\r\nb\r\n"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"a"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"b"[..]));
        assert_eq!(r.next_line().unwrap(), None);
    }

    #[test]
    fn strips_leading_bom_only() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(b"a\nb\n");
        let mut r = LineReader::new(Cursor::new(data));
        assert_eq!(r.next_line().unwrap(), Some(&b"a"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"b"[..]));
    }

    #[test]
    fn preserves_blank_lines() {
        let mut r = LineReader::new(Cursor::new(&b"a\n\nb\n"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"a"[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b""[..]));
        assert_eq!(r.next_line().unwrap(), Some(&b"b"[..]));
    }

    #[test]
    fn empty_input_yields_none() {
        let mut r = LineReader::new(Cursor::new(&b""[..]));
        assert_eq!(r.next_line().unwrap(), None);
    }

    #[test]
    fn tracks_line_number() {
        let mut r = LineReader::new(Cursor::new(&b"a\nb\nc\n"[..]));
        assert_eq!(r.line_number(), 0);
        r.next_line().unwrap();
        assert_eq!(r.line_number(), 1);
        r.next_line().unwrap();
        assert_eq!(r.line_number(), 2);
    }
}
