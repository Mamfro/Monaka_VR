//! Finding code in an x64 PE image by what it refers to: strings, globals and other functions. The
//! image is either a file's bytes or a loaded module's sections; both are read by RVA. Function
//! bounds come from the exception directory (`.pdata`), so every compiled function is known
//! without a symbol file. Instructions are decoded with [`crate::x64`].

use crate::pe::{Error, Result};
use crate::x64::{self, Instruction};
use std::collections::HashMap;
use std::sync::Mutex;

/// One section of the image, with its bytes as they are laid out from its RVA.
pub struct Section<'a> {
    pub name: String,
    pub rva: u32,
    pub bytes: &'a [u8],
    pub executable: bool,
}

/// What the section table says about one section.
#[derive(Clone, Debug)]
pub struct SectionHeader {
    pub name: String,
    pub rva: u32,
    pub virtual_size: u32,
    pub raw_pointer: u32,
    pub raw_size: u32,
    pub executable: bool,
}

/// One `.pdata` entry: a piece of a function (MSVC splits cold code into chained pieces).
#[derive(Clone, Copy, Debug)]
struct Chunk {
    start: u32,
    end: u32,
    /// The function this piece belongs to (its first piece's start).
    primary: u32,
}

pub struct Code<'a> {
    base: u64,
    sections: Vec<Section<'a>>,
    chunks: Vec<Chunk>,
    /// The other pieces of split functions, by function start.
    split: HashMap<u32, Vec<(u32, u32)>>,
    /// Functions referring to each UTF-16 string, once looked up ([`Self::prefetch_wide`]).
    wide: Mutex<HashMap<String, Vec<(u32, u32)>>>,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(Error("header cut short"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(Error("header cut short"))
}

/// The section table, the preferred image base and the exception directory, from the headers
/// (the first page of a file or a loaded image).
pub fn section_headers(headers: &[u8]) -> Result<(Vec<SectionHeader>, u64, (u32, u32))> {
    if u16_at(headers, 0)? != 0x5a4d {
        return Err(Error("no MZ header"));
    }
    let nt = u32_at(headers, 0x3c)? as usize;
    if nt > 0x1000 || u32_at(headers, nt)? != 0x4550 {
        return Err(Error("no PE header"));
    }
    let count = u16_at(headers, nt + 6)? as usize;
    let optional_size = u16_at(headers, nt + 20)? as usize;
    let optional = nt + 24;
    if u16_at(headers, nt + 4)? != 0x8664 || u16_at(headers, optional)? != 0x20b {
        return Err(Error("not an x64 PE image"));
    }
    if count > 96 || u32_at(headers, optional + 108)? < 4 {
        return Err(Error("implausible PE header"));
    }
    let base = u32_at(headers, optional + 24)? as u64 | (u32_at(headers, optional + 28)? as u64) << 32;
    let exceptions = (u32_at(headers, optional + 112 + 3 * 8)?, u32_at(headers, optional + 112 + 3 * 8 + 4)?);
    let mut sections = Vec::with_capacity(count);
    for i in 0..count {
        let at = optional + optional_size + i * 40;
        let raw_name = headers.get(at..at + 8).ok_or(Error("section table cut short"))?;
        sections.push(SectionHeader {
            name: String::from_utf8_lossy(raw_name).trim_end_matches('\0').to_owned(),
            virtual_size: u32_at(headers, at + 8)?,
            rva: u32_at(headers, at + 12)?,
            raw_size: u32_at(headers, at + 16)?,
            raw_pointer: u32_at(headers, at + 20)?,
            executable: u32_at(headers, at + 36)? & 0x2000_0000 != 0,
        });
    }
    Ok((sections, base, exceptions))
}

impl<'a> Code<'a> {
    /// A PE file's bytes. Pointers stored in it are taken at its preferred base.
    pub fn from_file(file: &'a [u8]) -> Result<Self> {
        let (headers, base, exceptions) = section_headers(file)?;
        let sections = headers
            .into_iter()
            .filter_map(|h| {
                let bytes = file.get(h.raw_pointer as usize..h.raw_pointer as usize + h.raw_size as usize)?;
                Some(Section { name: h.name, rva: h.rva, bytes, executable: h.executable })
            })
            .collect();
        Self::new(base, sections, exceptions)
    }

    /// A loaded image at `base`, its sections handed over by the caller (which checks they are
    /// readable), and the exception directory from its headers.
    pub fn new(base: u64, sections: Vec<Section<'a>>, exceptions: (u32, u32)) -> Result<Self> {
        let mut code = Self { base, sections, chunks: Vec::new(), split: HashMap::new(), wide: Mutex::new(HashMap::new()) };
        let (rva, size) = exceptions;
        let table = code.bytes(rva, size as usize).ok_or(Error("exception directory unreadable"))?;
        let mut raw = Vec::with_capacity(table.len() / 12);
        for entry in table.chunks_exact(12) {
            let field = |i: usize| u32::from_le_bytes([entry[i], entry[i + 1], entry[i + 2], entry[i + 3]]);
            raw.push((field(0), field(4), field(8)));
        }
        let chunks = raw
            .iter()
            .map(|&(start, end, unwind)| Chunk { start, end, primary: code.primary(start, unwind).unwrap_or(start) })
            .collect::<Vec<_>>();
        if chunks.windows(2).any(|w| w[0].start > w[1].start) {
            return Err(Error("exception directory is not sorted"));
        }
        for chunk in chunks.iter().filter(|c| c.primary != c.start) {
            code.split.entry(chunk.primary).or_default().push((chunk.start, chunk.end));
        }
        code.chunks = chunks;
        Ok(code)
    }

    /// Follows chained unwind information back to the function's first piece.
    fn primary(&self, mut start: u32, mut unwind: u32) -> Option<u32> {
        for _ in 0..32 {
            let info = self.bytes(unwind & !1, 4)?;
            if info[0] >> 3 & 4 == 0 {
                return Some(start);
            }
            let codes = info[2] as u32;
            let chained = (unwind & !1) + 4 + ((codes + 1) & !1) * 2;
            let next = self.bytes(chained, 12)?;
            start = u32::from_le_bytes([next[0], next[1], next[2], next[3]]);
            unwind = u32::from_le_bytes([next[8], next[9], next[10], next[11]]);
        }
        None
    }

    /// The address the image's stored pointers are relative to.
    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn sections(&self) -> &[Section<'a>] {
        &self.sections
    }

    /// `length` bytes at `rva`, all inside one section.
    pub fn bytes(&self, rva: u32, length: usize) -> Option<&'a [u8]> {
        let section = self.sections.iter().find(|s| rva >= s.rva && ((rva - s.rva) as usize) < s.bytes.len())?;
        let at = (rva - section.rva) as usize;
        section.bytes.get(at..at + length)
    }

    pub fn u32_at(&self, rva: u32) -> Option<u32> {
        self.bytes(rva, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A stored pointer at `rva`, as an RVA of this image (`None` if it points elsewhere).
    pub fn pointer_at(&self, rva: u32) -> Option<u32> {
        let b = self.bytes(rva, 8)?;
        let value = u64::from_le_bytes(b.try_into().ok()?);
        let offset = value.checked_sub(self.base)?;
        (offset < u32::MAX as u64).then_some(offset as u32).filter(|&r| self.bytes(r, 1).is_some())
    }

    pub fn is_executable(&self, rva: u32) -> bool {
        self.sections.iter().any(|s| s.executable && rva >= s.rva && ((rva - s.rva) as usize) < s.bytes.len())
    }

    pub fn decode(&self, rva: u32) -> Option<Instruction> {
        x64::decode(self.bytes(rva, 15).or_else(|| {
            // Near the end of a section: decode what is there.
            let section = self.sections.iter().find(|s| rva >= s.rva && ((rva - s.rva) as usize) < s.bytes.len())?;
            section.bytes.get((rva - section.rva) as usize..)
        })?)
    }

    fn chunk_index(&self, rva: u32) -> Option<usize> {
        let index = self.chunks.partition_point(|c| c.start <= rva).checked_sub(1)?;
        let chunk = self.chunks[index];
        (rva < chunk.end).then_some(index)
    }

    /// The start of the function whose code contains `rva` (its first piece, if it was split).
    pub fn function(&self, rva: u32) -> Option<u32> {
        self.chunk_index(rva).map(|i| self.chunks[i].primary)
    }

    /// Whether a function begins exactly at `rva`.
    pub fn is_function_start(&self, rva: u32) -> bool {
        self.chunk_index(rva).is_some_and(|i| self.chunks[i].start == rva && self.chunks[i].primary == rva)
    }

    /// How many functions the exception directory lists (pieces of split functions included).
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The `(start, end)` of every piece of the function that starts at `function`.
    pub fn pieces(&self, function: u32) -> Vec<(u32, u32)> {
        let Some(first) = self.chunk_index(function).filter(|&i| self.chunks[i].start == function) else { return Vec::new() };
        let mut pieces = vec![(self.chunks[first].start, self.chunks[first].end)];
        pieces.extend(self.split.get(&function).into_iter().flatten().copied());
        pieces.sort_unstable();
        pieces
    }

    /// Every `(start, end)` piece in address order, for whole-image passes.
    pub fn all_pieces(&self) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
        self.chunks.iter().map(|c| (c.start, c.end, c.primary))
    }

    /// The instructions of the piece `[start, end)`, decoded from its start; stops early at the
    /// first byte sequence the decoder refuses (data inside code, such as a jump table).
    pub fn linear(&self, start: u32, end: u32) -> Vec<(u32, Instruction)> {
        let mut out = Vec::new();
        let mut at = start;
        while at < end {
            let Some(instruction) = self.decode(at) else { break };
            out.push((at, instruction));
            at += instruction.length as u32;
        }
        out
    }

    /// The instructions of a whole function: every piece, in address order. A leaf function
    /// with no `.pdata` entry (it touches no stack) is decoded from its start to its first
    /// instruction that does not fall through, within 256 instructions.
    pub fn instructions(&self, function: u32) -> Vec<(u32, Instruction)> {
        let pieces = self.pieces(function);
        if pieces.is_empty() && self.chunk_index(function).is_none() && self.is_executable(function) {
            let mut out = Vec::new();
            let mut at = function;
            while out.len() < 256 {
                let Some(instruction) = self.decode(at) else { break };
                out.push((at, instruction));
                if instruction.ends_flow() {
                    break;
                }
                at += instruction.length as u32;
            }
            return out;
        }
        pieces.into_iter().flat_map(|(start, end)| self.linear(start, end)).collect()
    }

    /// The instruction that `rva` falls inside, decoding its function piece from the start.
    pub fn instruction_containing(&self, rva: u32) -> Option<(u32, Instruction)> {
        let chunk = self.chunks[self.chunk_index(rva)?];
        let mut at = chunk.start;
        while at < chunk.end {
            let instruction = self.decode(at)?;
            if rva < at + instruction.length as u32 {
                return Some((at, instruction));
            }
            at += instruction.length as u32;
        }
        None
    }

    /// Where NUL-terminated UTF-16 copies of `text` sit in data sections. A copy may be the tail of
    /// a longer string; code that refers to that address uses exactly `text`. (Functions referring
    /// to a string also count those loading a log record that points at it.)
    pub fn wide_strings(&self, text: &str) -> Vec<u32> {
        let mut pattern: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        pattern.extend([0, 0]);
        self.whole_strings(&pattern, 2)
    }

    /// Starts of NUL-terminated byte-string copies of `text` in data sections.
    pub fn ascii_strings(&self, text: &str) -> Vec<u32> {
        let mut pattern = text.as_bytes().to_vec();
        pattern.push(0);
        self.whole_strings(&pattern, 1)
    }

    fn whole_strings(&self, pattern: &[u8], unit: usize) -> Vec<u32> {
        let mut found = Vec::new();
        for section in self.sections.iter().filter(|s| !s.executable) {
            let bytes = section.bytes;
            let mut from = 0;
            while let Some(at) = find(&bytes[from..], pattern).map(|i| i + from) {
                if at % unit == 0 {
                    found.push(section.rva + at as u32);
                }
                from = at + 1;
            }
        }
        found
    }

    /// Instructions whose rip-relative operand refers to one of `targets`, as `(target,
    /// instruction)` pairs. Each hit is confirmed by decoding its function from the start.
    pub fn references(&self, targets: &[u32]) -> Vec<(u32, u32)> {
        let mut sorted = targets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let (Some(&low), Some(&high)) = (sorted.first(), sorted.last()) else { return Vec::new() };
        let mut hits = Vec::new();
        for section in self.sections.iter().filter(|s| s.executable) {
            let bytes = section.bytes;
            for at in 1..bytes.len().saturating_sub(4) {
                // A ModRM byte with mod = 00 and rm = 101 precedes a rip-relative displacement.
                if bytes[at - 1] & 0xc7 != 0x05 {
                    continue;
                }
                let displacement = i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
                // The target counts from the instruction's end, after up to 4 immediate bytes.
                let end = section.rva as i64 + at as i64 + 4;
                let near = end + displacement as i64;
                if near + 4 < low as i64 || near > high as i64 {
                    continue;
                }
                let first = sorted.partition_point(|&t| (t as i64) < near);
                if sorted.get(first).is_none_or(|&t| t as i64 > near + 4) {
                    continue;
                }
                let site = section.rva + at as u32;
                if let Some((start, instruction)) = self.instruction_containing(site)
                    && instruction.memory.is_some_and(|m| m.rip_relative && start + m.displacement_offset as u32 == site)
                    && let Some(target) = instruction.rip_target(start as u64).map(|t| t as u32)
                    && sorted.binary_search(&target).is_ok()
                {
                    hits.push((target, start));
                }
            }
        }
        hits
    }

    /// `call rel32` instructions that call `target`, confirmed by decoding their functions.
    pub fn callers(&self, target: u32) -> Vec<u32> {
        let mut hits = Vec::new();
        for section in self.sections.iter().filter(|s| s.executable) {
            let bytes = section.bytes;
            for at in 0..bytes.len().saturating_sub(5) {
                if bytes[at] != 0xe8 {
                    continue;
                }
                let relative = i32::from_le_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]]);
                let site = section.rva + at as u32;
                if (site as i64 + 5 + relative as i64) != target as i64 {
                    continue;
                }
                if self.instruction_containing(site).is_some_and(|(start, i)| start == site && i.is_call()) {
                    hits.push(site);
                }
            }
        }
        hits
    }

    /// Places in data sections that store a pointer to `target` (vtable slots, tables).
    pub fn pointers_to(&self, target: u32) -> Vec<u32> {
        let value = (self.base + target as u64).to_le_bytes();
        let mut hits = Vec::new();
        for section in self.sections.iter().filter(|s| !s.executable) {
            let mut from = 0;
            while let Some(at) = find(&section.bytes[from..], &value).map(|i| i + from) {
                if at % 8 == 0 {
                    hits.push(section.rva + at as u32);
                }
                from = at + 1;
            }
        }
        hits
    }

    /// The functions that refer to the UTF-16 string `text`, each once, in address order, with
    /// one reference site each.
    pub fn functions_referring_to_wide(&self, text: &str) -> Vec<(u32, u32)> {
        if let Some(known) = self.wide.lock().unwrap_or_else(|e| e.into_inner()).get(text) {
            return known.clone();
        }
        self.prefetch_wide(&[text]);
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).get(text).cloned().unwrap_or_default()
    }

    /// Where `strings` are referred to from data: an 8-aligned pointer to one of them, as
    /// `(string, place)`. Log calls in newer engines load a static log record whose first field
    /// points at the format string, rather than the string itself.
    fn records_of(&self, strings: &[u32]) -> Vec<(u32, u32)> {
        let mut wanted: Vec<u64> = strings.iter().map(|&s| self.base + s as u64).collect();
        wanted.sort_unstable();
        let mut out = Vec::new();
        for section in self.sections.iter().filter(|s| !s.executable) {
            for (i, chunk) in section.bytes.chunks_exact(8).enumerate() {
                let value = u64::from_le_bytes(chunk.try_into().unwrap());
                if wanted.binary_search(&value).is_ok() {
                    out.push(((value - self.base) as u32, section.rva + i as u32 * 8));
                }
            }
        }
        out
    }

    /// Looks up the functions referring to each of `texts` in one pass over the code (each lookup
    /// alone is a pass), for [`Self::functions_referring_to_wide`] to answer from.
    pub fn prefetch_wide(&self, texts: &[&str]) {
        let mut strings: Vec<(&str, Vec<u32>)> = texts.iter().map(|&t| (t, self.wide_strings(t))).collect();
        // A log record pointing at the string stands for it.
        let records = self.records_of(&strings.iter().flat_map(|(_, at)| at.iter().copied()).collect::<Vec<_>>());
        for (_, at) in strings.iter_mut() {
            let extra: Vec<u32> = records.iter().filter(|(string, _)| at.contains(string)).map(|(_, record)| *record).collect();
            at.extend(extra);
        }
        let all: Vec<u32> = strings.iter().flat_map(|(_, at)| at.iter().copied()).collect();
        let references = self.references(&all);
        let mut cache = self.wide.lock().unwrap_or_else(|e| e.into_inner());
        for (text, at) in strings {
            let sites: Vec<(u32, u32)> = references.iter().filter(|(target, _)| at.contains(target)).copied().collect();
            let mut functions: Vec<(u32, u32)> = sites.into_iter().filter_map(|(_, site)| Some((self.function(site)?, site))).collect();
            functions.sort_unstable();
            functions.dedup_by_key(|(function, _)| *function);
            cache.insert(text.to_owned(), functions);
        }
    }

    /// [`Self::functions_referring_to_wide`] for a byte string.
    pub fn functions_referring_to_ascii(&self, text: &str) -> Vec<(u32, u32)> {
        self.functions_referring_to(&self.ascii_strings(text))
    }

    fn functions_referring_to(&self, targets: &[u32]) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = self.references(targets).into_iter().filter_map(|(_, site)| Some((self.function(site)?, site))).collect();
        out.sort_unstable();
        out.dedup_by_key(|(function, _)| *function);
        out
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut from = 0;
    while from + needle.len() <= haystack.len() {
        let at = haystack[from..=haystack.len() - needle.len()].iter().position(|&b| b == first)? + from;
        if &haystack[at..at + needle.len()] == needle {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_whole_strings_and_references_in_a_system_dll() {
        // The x64 file even from a 32-bit test (whose System32 is redirected to the 32-bit one).
        let folder = if cfg!(target_arch = "x86") { "Sysnative" } else { "System32" };
        let bytes = std::fs::read(format!(r"C:\Windows\{folder}\kernelbase.dll")).expect("kernelbase.dll is readable");
        let code = Code::from_file(&bytes).unwrap();
        assert!(code.chunk_count() > 1000);
        let strings = code.wide_strings("kernel32.dll");
        assert!(!strings.is_empty(), "kernelbase names kernel32");
        let references = code.references(&strings);
        for &(target, site) in &references {
            assert!(strings.contains(&target));
            assert!(code.function(site).is_some(), "every confirmed reference is inside a function");
        }
    }

    /// How much of a game executable decodes cleanly, function by function. Run with
    /// `MONAKA_SCAN_EXE=<path> cargo test -p monaka_core --release whole_functions -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a game executable"]
    fn whole_functions_decode_to_their_ends() {
        let path = std::env::var("MONAKA_SCAN_EXE").expect("MONAKA_SCAN_EXE");
        let bytes = std::fs::read(path).unwrap();
        let code = Code::from_file(&bytes).unwrap();
        let (mut exact, mut short, mut over) = (0usize, 0usize, 0usize);
        let mut samples = Vec::new();
        for (start, end, _) in code.all_pieces() {
            let decoded = code.linear(start, end);
            let reached = decoded.last().map_or(start, |(at, i)| at + i.length as u32);
            match reached.cmp(&end) {
                std::cmp::Ordering::Equal => exact += 1,
                std::cmp::Ordering::Less => {
                    short += 1;
                    if samples.len() < 12 {
                        samples.push((start, reached, code.bytes(reached, 8).map(|b| b.to_vec())));
                    }
                }
                std::cmp::Ordering::Greater => over += 1,
            }
        }
        let total = exact + short + over;
        println!("{total} pieces: {exact} exact, {short} stopped early, {over} ran past the end");
        for (start, at, bytes) in samples {
            println!("  piece {start:#x} stopped at {at:#x}: {bytes:02x?}");
        }
        assert!(exact as f64 / total as f64 > 0.97, "the decoder accounts for nearly every function");
    }
}
