//! Peak memory stays flat: validating a large generated file of each format, resident memory
//! grows no further once the first megabytes are read.

use std::io::{self, Read};
use std::process::Command;

use hpt_validator::{Format, Options, validate};

/// This process's resident memory in KiB.
fn rss() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs on every Unix");
    let kib = String::from_utf8(out.stdout).expect("ps prints digits");
    kib.trim().parse().expect("ps prints the KiB")
}

/// `block` `left` times, made as it is read so the input takes no memory, sampling resident
/// memory at the start of each block.
struct Blocks<'a> {
    block: &'a [u8],
    left: usize,
    at: usize,
    rss: Vec<u64>,
}

impl Read for Blocks<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at == self.block.len() {
            if self.left == 0 {
                return Ok(0);
            }
            self.rss.push(rss());
            self.left -= 1;
            self.at = 0;
        }
        let n = buf.len().min(self.block.len() - self.at);
        buf[..n].copy_from_slice(&self.block[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Validates `head`, `chunk` repeated to 32 MiB, then `tail`; the file must be valid, and
/// resident memory past its first 4 MiB must stay within 1 MiB of its peak there.
fn flat(format: Format, head: &[u8], chunk: &[u8], tail: &[u8]) {
    let block = chunk.repeat((1 << 20) / chunk.len() + 1);
    let mut blocks = Blocks { block: &block, left: 32, at: block.len(), rss: Vec::new() };
    let input = head.chain(&mut blocks).chain(tail);
    let report = validate(input, format, "3.0.0", &Options::default());
    let report = report.expect("a known version, and generated input always reads");
    assert!(report.errors.is_empty() && report.alerts.is_empty(), "{report:?}");
    let (warm, rest) = blocks.rss.split_at(4);
    let warm = warm.iter().max().expect("four samples");
    assert!(rest.iter().all(|r| *r <= warm + 1024), "KiB per MiB read: {:?}", blocks.rss);
}

/// Both formats in one test: resident memory is the whole process's.
#[test]
fn peak_memory_is_flat() {
    let case = include_bytes!("cases/findings/template-tall-3.0.0.csv");
    // Three header rows, then the data row to repeat.
    let rows = case.iter().enumerate().filter(|(_, b)| **b == b'\n');
    let at = rows.map(|(i, _)| i + 1).nth(2).unwrap();
    flat(Format::Csv, &case[..at], &case[at..], b"");

    let case = include_str!("cases/upstream/3.0/sample-valid.json");
    // Everything up to the end of the charge items, then the items again after a comma.
    let list = "\"standard_charge_information\": [";
    let (start, end) = (case.find(list).unwrap() + list.len(), case.rfind(']').unwrap());
    let items = format!(",{}", &case[start..end]);
    flat(Format::Json, &case.as_bytes()[..end], items.as_bytes(), &case.as_bytes()[end..]);
}
