//! papaparse 5.5.3's `Parser.parse` loop for upstream's configuration (`delimiter: ","`,
//! `newline: "\n"`, quote and escape `"`), fed the way a Node file stream feeds it
//! (`js::Stream`): upstream's `removeBOM` on the first chunk only, and an unfinished row parsed
//! again once the next chunk arrives.

use crate::{Code, js};
use std::io::{self, Read};

/// One row as `Reader::next_row` lends it, its fields borrowed from the reader's text.
pub struct Row<'a> {
    /// Physical line where the row starts, 1-based, counting lines removed with the BOM.
    pub line: u64,
    /// Quote problems papaparse reports for this row, in its order, with the field each
    /// quoted field starts at.
    pub quotes: &'a [(Code, usize)],
    /// The text from the row's start on.
    text: &'a str,
    spans: &'a [(usize, usize, usize)],
    unescaped: &'a str,
}

impl<'a> Row<'a> {
    /// The fields that are not empty, with their index; most fields of a wide file are.
    pub fn non_empty(self) -> impl Iterator<Item = (usize, &'a str)> {
        let (text, unescaped) = (self.text, self.unescaped);
        self.spans.iter().map(move |&(i, start, end)| {
            let field = if end <= text.len() {
                &text[start..end]
            } else {
                &unescaped[start - text.len()..end - text.len()]
            };
            (i, field)
        })
    }
}

/// A row's fields and quote problems, the buffers reused for the next row, so reading
/// allocates only for a longer row.
#[derive(Default)]
struct Parsed {
    fields: usize,
    /// Each non-empty field's index and byte range in the text from the row's start on; past
    /// the text's end, in `unescaped` after the text's length.
    spans: Vec<(usize, usize, usize)>,
    /// Quoted fields with `""`, read as `"`, back to back.
    unescaped: String,
    quotes: Vec<(Code, usize)>,
}

impl Parsed {
    fn push(&mut self, start: usize, end: usize) {
        if start < end {
            self.spans.push((self.fields, start, end));
        }
        self.fields += 1;
    }

    /// The quoted field `s[start..end]`; `doubled` tells whether it holds any `""`.
    fn push_quoted(&mut self, s: &str, start: usize, end: usize, doubled: bool) {
        if !doubled {
            return self.push(start, end);
        }
        let from = s.len() + self.unescaped.len();
        let mut field = &s[start..end];
        // A char search: splitting on `""` would set up a substring search for every field.
        while let Some(i) = field.find('"') {
            self.unescaped.push_str(&field[..=i]);
            let rest = &field[i + 1..];
            field = rest.strip_prefix('"').unwrap_or(rest);
        }
        self.unescaped.push_str(field);
        self.push(from, s.len() + self.unescaped.len());
    }
}

pub struct Reader<R> {
    stream: js::Stream<R>,
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
    parsed: Parsed,
}

impl<R: Read> Reader<R> {
    pub fn new(input: R) -> Self {
        Self::with_chunk(input, js::CHUNK)
    }

    pub fn with_chunk(input: R, chunk: usize) -> Self {
        Reader {
            stream: js::Stream::new(input, chunk),
            text: String::new(),
            pos: 0,
            scanned: 0,
            first: true,
            eof: false,
            line: 1,
            bom_lines: 0,
            parsed: Parsed::default(),
        }
    }

    pub fn next_row(&mut self) -> io::Result<Option<Row<'_>>> {
        loop {
            // Before the end of input a row ends only at a newline; a row not tried yet needs
            // no search for one, as parse_row finds out.
            // ponytail: a quoted field spanning chunks with newlines is parsed again per chunk,
            // as papaparse does; make parse_row resumable if such files show up
            if (self.eof
                || self.scanned == self.pos
                || self.text.as_bytes()[self.scanned..].contains(&b'\n'))
                && let Some((len, lines)) =
                    parse_row(&self.text[self.pos..], self.eof, &mut self.parsed)
            {
                let (line, start) = (self.line, self.pos);
                self.line += lines;
                self.pos += len;
                self.scanned = self.pos;
                return Ok(Some(Row {
                    line,
                    quotes: &self.parsed.quotes,
                    text: &self.text[start..],
                    spans: &self.parsed.spans,
                    unescaped: &self.parsed.unescaped,
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
        self.text.drain(..self.pos);
        self.scanned -= self.pos;
        self.pos = 0;
        if !self.stream.next(&mut self.text)? {
            self.eof = true;
        } else if std::mem::take(&mut self.first) {
            let cut = self.text.len() - crate::remove_bom(&self.text).len();
            self.bom_lines = self.text[..cut].matches('\n').count() as u64;
            self.line += self.bom_lines;
            self.text.drain(..cut);
        }
        Ok(())
    }
}

/// One row of papaparse's parse loop at the start of `s`, into `row`. Returns the row's
/// length in bytes and the newlines in it, or `None` when `s` is empty, or when the row may
/// continue past the end of `s` and more input is coming (papaparse's `ignoreLastRow`).
fn parse_row(s: &str, eof: bool, row: &mut Parsed) -> Option<(usize, u64)> {
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    // `str::find` with a char runs std's memchr, which wins on rows and long fields.
    let newline_from = |from: usize| s[from..].find('\n').map(|i| from + i);
    row.fields = 0;
    row.spans.clear();
    row.unescaped.clear();
    row.quotes.clear();
    let mut cursor = 0;
    let first = newline_from(0);
    let mut newline = first;
    // The first newline ends the row, unless a quoted field holds it.
    let counted =
        |len: usize| (len, first.map_or(0, |n| 1 + s[n + 1..len].matches('\n').count() as u64));
    loop {
        if b.get(cursor) == Some(&b'"') {
            let start = cursor + 1;
            let mut q = cursor;
            let mut doubled = false;
            loop {
                let Some(next) = quote_from(s, q + 1) else {
                    if !eof {
                        return None;
                    }
                    // No closing quote: the rest of the input is the field, quotes as written.
                    row.quotes.push((Code::XCsvMissingQuotes, row.fields));
                    row.push(start, s.len());
                    return Some(counted(s.len()));
                };
                q = next;
                if q == s.len() - 1 {
                    if !eof {
                        return None;
                    }
                    row.push_quoted(s, start, q, doubled);
                    return Some(counted(s.len()));
                }
                if b[q + 1] == b'"' {
                    q += 1;
                    doubled = true;
                    continue;
                }
                // A closing quote is followed by optional JS whitespace and `,` or `\n`.
                let after = &s[q + 1..];
                let end = if matches!(b[q + 1], b',' | b'\n') {
                    q + 1
                } else {
                    q + 1 + after.len()
                        - after.trim_start_matches(|c| c != '\n' && js::is_space(c)).len()
                };
                match b.get(end) {
                    Some(&c @ (b',' | b'\n')) => {
                        row.push_quoted(s, start, q, doubled);
                        if c == b'\n' {
                            return Some(counted(end + 1));
                        }
                        cursor = end + 1;
                        break;
                    }
                    _ => row.quotes.push((Code::XCsvInvalidQuotes, row.fields)),
                }
            }
            continue;
        }
        // Most cells of a wide file are empty, many of them in a row.
        let empty = b[cursor..].iter().take_while(|&&c| c == b',').count();
        if empty > 0 {
            row.fields += empty;
            cursor += empty;
            continue;
        }
        if newline.is_some_and(|n| n < cursor) {
            newline = newline_from(cursor);
        }
        let field_end = newline.unwrap_or(b.len());
        if let Some(comma) = s[cursor..field_end].find(',') {
            row.push(cursor, cursor + comma);
            cursor += comma + 1;
            continue;
        }
        if let Some(n) = newline {
            row.push(cursor, n);
            return Some(counted(n + 1));
        }
        if !eof {
            return None;
        }
        row.push(cursor, s.len());
        return Some(counted(s.len()));
    }
}

/// The first `"` in `s` from byte `from` on. Most quoted fields are short, and a plain loop
/// finds their closing quote sooner; std's memchr wins on notes of a kilobyte.
fn quote_from(s: &str, from: usize) -> Option<usize> {
    let b = s.as_bytes();
    let near = (from + 128).min(b.len());
    if let Some(i) = b[from..near].iter().position(|&x| x == b'"') {
        return Some(from + i);
    }
    // The quote is ASCII, so the bytes of a char split at `near` are none.
    let far =
        (near..=b.len()).find(|&i| s.is_char_boundary(i)).expect("the end is a char boundary");
    s[far..].find('"').map(|i| far + i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Each row's fields, quote problems and line.
    type Rows = Vec<(Vec<String>, Vec<(Code, usize)>, u64)>;

    fn read(bytes: &[u8], chunk: usize) -> (Rows, u64) {
        let mut reader = Reader::with_chunk(bytes, chunk);
        let mut rows = Vec::new();
        while let Some(row) = reader.next_row().unwrap() {
            let (line, quotes) = (row.line, row.quotes.to_vec());
            let non_empty: Vec<(usize, String)> =
                row.non_empty().map(|(i, field)| (i, field.into())).collect();
            let mut fields = vec![String::new(); reader.parsed.fields];
            for (i, field) in non_empty {
                fields[i] = field;
            }
            rows.push((fields, quotes, line));
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
                    .flat_map(|(i, r)| r.1.iter().map(move |(c, _)| (i, c)))
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
                let fields: Vec<&Vec<String>> = rows.iter().map(|r| &r.0).collect();
                let got = json!({ "rows": fields, "errors": errors });
                assert!(got == golden[chunk.to_string()], "{} at chunk {chunk}", path.display());
            }
            cases += 1;
        }
        assert!(cases > 0);
    }

    #[test]
    fn physical_lines() {
        let lines =
            |bytes: &[u8], chunk| read(bytes, chunk).0.iter().map(|r| r.2).collect::<Vec<_>>();
        assert_eq!(lines(b"a,b\n\n\"c\n\nd\"\ne\n", 65536), [1, 2, 3, 6]);
        assert_eq!(lines("\u{feff}\n\n\na,b\n".as_bytes(), 65536), [4]);
        assert_eq!(lines("\u{feff}\n\n\na,b\n".as_bytes(), 1), [1, 2, 3, 4]);
        assert_eq!(read("\u{feff}\n\n\na,b\n".as_bytes(), 65536).1, 3);
    }
}
