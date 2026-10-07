//! papaparse 5.5.3's `Parser.parse` loop for upstream's configuration (`delimiter: ","`,
//! `newline: "\n"`, quote and escape `"`), fed the way a Node file stream feeds it: fixed-size
//! byte chunks decoded as UTF-8, a BOM `trimStart` on the first non-empty chunk only, and an
//! unfinished row parsed again once the next chunk arrives.

use crate::{Code, js};
use std::io::{self, Read};

/// `fs.createReadStream`'s chunk size, which bounds how far the BOM trim reaches.
pub const CHUNK: usize = 64 * 1024;

#[derive(Debug, PartialEq)]
pub struct Row {
    pub fields: Vec<String>,
    /// Physical line where the row starts, 1-based, counting lines removed with the BOM.
    pub line: u64,
    /// Quote problems papaparse reports for this row, in its order.
    pub quotes: Vec<Code>,
}

pub struct Reader<R> {
    input: R,
    chunk: u64,
    /// Bytes of a UTF-8 sequence split by the chunk boundary.
    undecoded: Vec<u8>,
    /// Decoded text; rows before `pos` have been returned.
    text: String,
    pos: usize,
    /// Text after `pos` already parsed without finishing a row.
    scanned: usize,
    first: bool,
    eof: bool,
    line: u64,
    /// Newlines removed together with the BOM.
    pub bom_lines: u64,
}

impl<R: Read> Reader<R> {
    pub fn new(input: R) -> Self {
        Self::with_chunk(input, CHUNK)
    }

    pub fn with_chunk(input: R, chunk: usize) -> Self {
        Reader {
            input,
            chunk: chunk as u64,
            undecoded: Vec::new(),
            text: String::new(),
            pos: 0,
            scanned: 0,
            first: true,
            eof: false,
            line: 1,
            bom_lines: 0,
        }
    }

    pub fn next_row(&mut self) -> io::Result<Option<Row>> {
        loop {
            // Before the end of input a row ends only at a newline.
            // ponytail: a quoted field spanning chunks with newlines is parsed again per chunk,
            // as papaparse does; make parse_row resumable if such files show up
            if (self.eof || self.text.as_bytes()[self.scanned..].contains(&b'\n'))
                && let Some((fields, quotes, len)) = parse_row(&self.text[self.pos..], self.eof)
            {
                let line = self.line;
                self.line += self.text[self.pos..self.pos + len].matches('\n').count() as u64;
                self.pos += len;
                self.scanned = self.pos;
                return Ok(Some(Row {
                    fields,
                    line,
                    quotes,
                }));
            }
            self.scanned = self.text.len();
            if self.eof {
                return Ok(None);
            }
            self.read_chunk()?;
        }
    }

    fn read_chunk(&mut self) -> io::Result<()> {
        let n = self
            .input
            .by_ref()
            .take(self.chunk)
            .read_to_end(&mut self.undecoded)?;
        self.text.drain(..self.pos);
        self.scanned -= self.pos;
        self.pos = 0;
        let start = self.text.len();
        if n == 0 {
            self.eof = true;
            if !self.undecoded.is_empty() {
                self.undecoded.clear();
                self.text.push('\u{fffd}');
            }
        } else {
            decode(&mut self.undecoded, &mut self.text);
        }
        // Node emits no empty chunks, so the first chunk is the first non-empty one.
        if self.first && self.text.len() > start {
            self.first = false;
            if self.text.starts_with('\u{feff}') {
                let cut = self.text.len() - js::trim_start(&self.text).len();
                self.bom_lines = self.text[..cut].matches('\n').count() as u64;
                self.line += self.bom_lines;
                self.text.drain(..cut);
            }
        }
        Ok(())
    }
}

/// Node's streaming UTF-8 decoder: complete sequences go to `out`, each maximal invalid
/// subpart becomes U+FFFD, and an incomplete sequence at the end stays in `bytes`.
fn decode(bytes: &mut Vec<u8>, out: &mut String) {
    let incomplete = bytes.utf8_chunks().last().map_or(0, |c| {
        let bad = c.invalid();
        let cut_short = std::str::from_utf8(bad).is_err_and(|e| e.error_len().is_none());
        if cut_short { bad.len() } else { 0 }
    });
    let done = bytes.len() - incomplete;
    out.push_str(&String::from_utf8_lossy(&bytes[..done]));
    bytes.drain(..done);
}

/// A quoted field's text, `""` read as `"`.
fn unescape(field: &str) -> String {
    if field.contains('"') {
        field.replace("\"\"", "\"")
    } else {
        field.to_string()
    }
}

/// One row of papaparse's parse loop at the start of `s`: its fields, quote problems and
/// length in bytes. `None` when `s` is empty, or when the row may continue past the end of
/// `s` and more input is coming (papaparse's `ignoreLastRow`).
fn parse_row(s: &str, eof: bool) -> Option<(Vec<String>, Vec<Code>, usize)> {
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    let find = |c: u8, from: usize| b[from..].iter().position(|&x| x == c).map(|i| from + i);
    let mut fields = Vec::new();
    let mut quotes = Vec::new();
    let mut cursor = 0;
    let mut newline = find(b'\n', 0);
    loop {
        if b.get(cursor) == Some(&b'"') {
            let start = cursor + 1;
            let mut q = cursor;
            loop {
                let Some(next) = find(b'"', q + 1) else {
                    if !eof {
                        return None;
                    }
                    // No closing quote: the rest of the input is the field, quotes as written.
                    quotes.push(Code::XCsvMissingQuotes);
                    fields.push(s[start..].to_string());
                    return Some((fields, quotes, s.len()));
                };
                q = next;
                if q == s.len() - 1 {
                    if !eof {
                        return None;
                    }
                    fields.push(unescape(&s[start..q]));
                    return Some((fields, quotes, s.len()));
                }
                if b[q + 1] == b'"' {
                    q += 1;
                    continue;
                }
                // A closing quote is followed by optional JS whitespace and `,` or `\n`.
                let after = &s[q + 1..];
                let end = q + 1 + after.len()
                    - after
                        .trim_start_matches(|c| c != '\n' && js::is_space(c))
                        .len();
                match b.get(end) {
                    Some(&c @ (b',' | b'\n')) => {
                        fields.push(unescape(&s[start..q]));
                        if c == b'\n' {
                            return Some((fields, quotes, end + 1));
                        }
                        cursor = end + 1;
                        break;
                    }
                    _ => quotes.push(Code::XCsvInvalidQuotes),
                }
            }
            continue;
        }
        if newline.is_some_and(|n| n < cursor) {
            newline = find(b'\n', cursor);
        }
        let field_end = newline.unwrap_or(b.len());
        if let Some(comma) = b[cursor..field_end].iter().position(|&x| x == b',') {
            fields.push(s[cursor..cursor + comma].to_string());
            cursor += comma + 1;
            continue;
        }
        if let Some(n) = newline {
            fields.push(s[cursor..n].to_string());
            return Some((fields, quotes, n + 1));
        }
        if !eof {
            return None;
        }
        fields.push(s[cursor..].to_string());
        return Some((fields, quotes, s.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn read(bytes: &[u8], chunk: usize) -> (Vec<Row>, u64) {
        let mut reader = Reader::with_chunk(bytes, chunk);
        let mut rows = Vec::new();
        while let Some(row) = reader.next_row().unwrap() {
            rows.push(row);
        }
        (rows, reader.bom_lines)
    }

    #[test]
    fn papaparse_rows() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/reader");
        let mut cases = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().unwrap() != "csv" {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let golden: Value = serde_json::from_str(
                &std::fs::read_to_string(path.with_extension("json")).unwrap(),
            )
            .unwrap();
            for chunk in [1, 1024, 65536] {
                let (rows, _) = read(&bytes, chunk);
                let errors: Vec<Value> = (rows.iter().enumerate())
                    .flat_map(|(i, r)| r.quotes.iter().map(move |c| (i, c)))
                    .map(|(i, c)| {
                        json!([
                            i,
                            if *c == Code::XCsvInvalidQuotes {
                                "InvalidQuotes"
                            } else {
                                "MissingQuotes"
                            }
                        ])
                    })
                    .collect();
                let fields: Vec<&Vec<String>> = rows.iter().map(|r| &r.fields).collect();
                let got = json!({ "rows": fields, "errors": errors });
                assert!(
                    got == golden[chunk.to_string()],
                    "{} at chunk {chunk}",
                    path.display()
                );
            }
            cases += 1;
        }
        assert!(cases > 0);
    }

    #[test]
    fn physical_lines() {
        let lines = |bytes: &[u8], chunk| {
            read(bytes, chunk)
                .0
                .iter()
                .map(|r| r.line)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(b"a,b\n\n\"c\n\nd\"\ne\n", 65536), [1, 2, 3, 6]);
        assert_eq!(lines("\u{feff}\n\n\na,b\n".as_bytes(), 65536), [4]);
        assert_eq!(lines("\u{feff}\n\n\na,b\n".as_bytes(), 1), [1, 2, 3, 4]);
        assert_eq!(read("\u{feff}\n\n\na,b\n".as_bytes(), 65536).1, 3);
    }
}
