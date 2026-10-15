//! Finding a named export in a PE image (x64, or 32-bit x86 for 32-bit games), either a file on
//! disk or an image mapped in another process. Every read is bounds-checked; anything unexpected is an error rather than a guess.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub &'static str);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Reads `buffer.len()` bytes at an RVA of the image; false when outside it or unreadable.
pub trait Image {
    fn read(&self, rva: u32, buffer: &mut [u8]) -> bool;
}

fn u16_at(image: &dyn Image, rva: u32) -> Result<u16> {
    let mut b = [0; 2];
    image.read(rva, &mut b).then(|| u16::from_le_bytes(b)).ok_or(Error("read outside the image"))
}

fn u32_at(image: &dyn Image, rva: u32) -> Result<u32> {
    let mut b = [0; 4];
    image.read(rva, &mut b).then(|| u32::from_le_bytes(b)).ok_or(Error("read outside the image"))
}

#[derive(Clone, Copy, Debug)]
struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    raw_pointer: u32,
    executable: bool,
}

struct Headers {
    sections: Vec<Section>,
    export_rva: u32,
    export_size: u32,
    image_size: u32,
}

/// Parses the headers through `read`, which takes file offsets for a file and RVAs for a mapped
/// image (headers sit at the same place in both).
fn headers(read: &dyn Image) -> Result<Headers> {
    if u16_at(read, 0)? != 0x5a4d {
        return Err(Error("no MZ header"));
    }
    let nt = u32_at(read, 0x3c)?;
    if nt > 0x1000 || u32_at(read, nt)? != 0x4550 {
        return Err(Error("no PE header"));
    }
    let machine = u16_at(read, nt + 4)?;
    let section_count = u16_at(read, nt + 6)? as u32;
    let optional_size = u16_at(read, nt + 20)? as u32;
    let optional = nt + 24;
    // Where the data directories start: PE32+ (x64) keeps 64-bit fields before them, PE32 (x86)
    // 32-bit ones, 16 bytes fewer.
    let directory_table = match (machine, u16_at(read, optional)?) {
        (0x8664, 0x20b) => 112,
        (0x14c, 0x10b) => 96,
        _ => return Err(Error("not an x64 or x86 PE image")),
    };
    if section_count > 96 || optional_size < directory_table + 8 {
        return Err(Error("implausible PE header"));
    }
    let image_size = u32_at(read, optional + 56)?;
    let directories = u32_at(read, optional + directory_table - 4)?;
    let (export_rva, export_size) =
        if directories > 0 { (u32_at(read, optional + directory_table)?, u32_at(read, optional + directory_table + 4)?) } else { (0, 0) };
    let mut sections = Vec::with_capacity(section_count as usize);
    for i in 0..section_count {
        let at = optional + optional_size + i * 40;
        sections.push(Section {
            virtual_size: u32_at(read, at + 8)?,
            virtual_address: u32_at(read, at + 12)?,
            raw_size: u32_at(read, at + 16)?,
            raw_pointer: u32_at(read, at + 20)?,
            executable: u32_at(read, at + 36)? & 0x2000_0000 != 0,
        });
    }
    Ok(Headers { sections, export_rva, export_size, image_size })
}

/// A PE file's bytes, read by RVA through its section table.
pub struct FileImage<'a> {
    bytes: &'a [u8],
    sections: Vec<Section>,
}

impl<'a> FileImage<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let sections = headers(&RawBytes(bytes))?.sections;
        Ok(Self { bytes, sections })
    }
}

struct RawBytes<'a>(&'a [u8]);

impl Image for RawBytes<'_> {
    fn read(&self, offset: u32, buffer: &mut [u8]) -> bool {
        let start = offset as usize;
        match self.0.get(start..start + buffer.len()) {
            Some(source) => {
                buffer.copy_from_slice(source);
                true
            }
            None => false,
        }
    }
}

impl Image for FileImage<'_> {
    fn read(&self, rva: u32, buffer: &mut [u8]) -> bool {
        let Some(section) = self.sections.iter().find(|s| rva >= s.virtual_address && rva - s.virtual_address < s.raw_size) else {
            return false;
        };
        let inside = rva - section.virtual_address;
        if buffer.len() as u64 > (section.raw_size - inside) as u64 {
            return false;
        }
        RawBytes(self.bytes).read(section.raw_pointer + inside, buffer)
    }
}

/// Whether a PE file is 32-bit x86 code (rather than x64).
pub fn file_is_x86(bytes: &[u8]) -> Result<bool> {
    headers(&RawBytes(bytes))?;
    Ok(u16_at(&RawBytes(bytes), u32_at(&RawBytes(bytes), 0x3c)? + 4)? == 0x14c)
}

/// The RVA of the named export of a PE file. Forwarded exports are refused.
pub fn file_export(bytes: &[u8], name: &str) -> Result<Option<u32>> {
    let parsed = headers(&RawBytes(bytes))?;
    export(&FileImage { bytes, sections: parsed.sections.clone() }, &parsed, name, false)
}

/// The NUL-terminated text a PE file exports as data under `name` (at most 260 bytes), for
/// example a producer's `MONAKA_HOST`.
pub fn file_export_text(bytes: &[u8], name: &str) -> Result<Option<String>> {
    file_export_text_up_to(bytes, name, 260)
}

/// [`file_export_text`] for text of up to `limit` bytes (a producer's `MONAKA_MANIFEST`).
pub fn file_export_text_up_to(bytes: &[u8], name: &str, limit: u32) -> Result<Option<String>> {
    let parsed = headers(&RawBytes(bytes))?;
    let image = FileImage { bytes, sections: parsed.sections.clone() };
    let Some(rva) = export(&image, &parsed, name, false)? else { return Ok(None) };
    let mut text = Vec::new();
    for offset in 0..limit {
        let mut byte = [0u8];
        if !image.read(rva + offset, &mut byte) {
            return Err(Error("exported text runs outside the file"));
        }
        if byte[0] == 0 {
            return String::from_utf8(text).map(Some).map_err(|_| Error("exported text is not UTF-8"));
        }
        text.push(byte[0]);
    }
    Err(Error("exported text is not terminated"))
}

/// The 32-bit number a PE file exports as data under `name` (a producer's `MONAKA_CONTRACT`).
pub fn file_export_u32(bytes: &[u8], name: &str) -> Result<Option<u32>> {
    let parsed = headers(&RawBytes(bytes))?;
    let image = FileImage { bytes, sections: parsed.sections.clone() };
    let Some(rva) = export(&image, &parsed, name, false)? else { return Ok(None) };
    let mut value = [0u8; 4];
    if !image.read(rva, &mut value) {
        return Err(Error("exported number runs outside the file"));
    }
    Ok(Some(u32::from_le_bytes(value)))
}

/// Whether `executable` is one of a producer's host names (`MONAKA_HOST`: one executable, or several
/// `;`-separated), ignoring case.
pub fn host_listed(hosts: &str, executable: &str) -> bool {
    // `*` (`manifest::ANY_EXECUTABLE`): a producer for any game on its engine checks the engine itself.
    hosts.split(';').any(|host| !host.is_empty() && !executable.is_empty() && (host == monaka_contract::manifest::ANY_EXECUTABLE || host.eq_ignore_ascii_case(executable)))
}

/// The RVA of the named export of an image mapped in memory (read through `image` by RVA), which
/// must be `image_size` bytes and point into an executable section.
pub fn mapped_export(image: &dyn Image, image_size: u32, name: &str) -> Result<Option<u32>> {
    let parsed = headers(image)?;
    if parsed.image_size != image_size {
        return Err(Error("mapped image size differs from its header"));
    }
    export(image, &parsed, name, true)
}

fn export(image: &dyn Image, headers: &Headers, name: &str, executable: bool) -> Result<Option<u32>> {
    if headers.export_rva == 0 {
        return Ok(None);
    }
    let directory = headers.export_rva;
    let functions_count = u32_at(image, directory + 20)?;
    let names_count = u32_at(image, directory + 24)?;
    if functions_count > 65536 || names_count > 65536 {
        return Err(Error("implausible export count"));
    }
    let functions = u32_at(image, directory + 28)?;
    let names = u32_at(image, directory + 32)?;
    let ordinals = u32_at(image, directory + 36)?;
    for i in 0..names_count {
        let name_rva = u32_at(image, names + 4 * i)?;
        let mut found = [0u8; 256];
        let wanted = name.as_bytes();
        if wanted.len() >= found.len() {
            return Err(Error("export name too long"));
        }
        // Read exactly the wanted length plus its terminator; a shorter name fails the compare.
        let compare = &mut found[..wanted.len() + 1];
        if !image.read(name_rva, compare) || &compare[..wanted.len()] != wanted || compare[wanted.len()] != 0 {
            continue;
        }
        let ordinal = u16_at(image, ordinals + 2 * i)? as u32;
        if ordinal >= functions_count {
            return Err(Error("invalid export ordinal"));
        }
        let rva = u32_at(image, functions + 4 * ordinal)?;
        if rva == 0 || (rva >= directory && rva - directory < headers.export_size) {
            return Err(Error("forwarded or empty export"));
        }
        if executable && !headers.sections.iter().any(|s| s.executable && rva >= s.virtual_address && rva - s.virtual_address < s.virtual_size) {
            return Err(Error("export is not in executable code"));
        }
        return Ok(Some(rva));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
