//! A file on the Windows clipboard, as Explorer's Copy puts it there (`CF_HDROP`): pasted into a
//! chat, a mail or a folder, the file itself goes, unchanged.
#![allow(unsafe_code)]

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// The clipboard format of a list of files.
const CF_HDROP: u32 = 15;
/// `DROPFILES`: where the names start (its own size), a drop point and a non-client flag (unused),
/// and whether the names are UTF-16.
const DROPFILES_SIZE: u32 = 20;

/// `path` as `CF_HDROP` data: the `DROPFILES` header, then the absolute path in UTF-16, ended by
/// one NUL and the list by another.
fn drop_files(path: &Path) -> Vec<u8> {
    let mut data = Vec::new();
    for field in [DROPFILES_SIZE, 0, 0, 0, 1] {
        data.extend_from_slice(&field.to_le_bytes());
    }
    for unit in path.as_os_str().encode_wide().chain([0, 0]) {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data
}

/// Puts the file at `path` on the clipboard, replacing what was there.
pub fn copy_file(path: &Path) -> Result<(), String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let data = drop_files(&path);
    // SAFETY: a global block of the data's size, written whole while locked; once the clipboard
    // takes it, it is the clipboard's, else freed here.
    unsafe {
        let block = GlobalAlloc(GMEM_MOVEABLE, data.len()).map_err(|e| e.to_string())?;
        let at = GlobalLock(block).cast::<u8>();
        if at.is_null() {
            let _ = GlobalFree(Some(block));
            return Err("the clipboard block could not be locked".into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), at, data.len());
        let _ = GlobalUnlock(block);
        if let Err(e) = OpenClipboard(None) {
            let _ = GlobalFree(Some(block));
            return Err(format!("the clipboard is busy: {e}"));
        }
        let set = EmptyClipboard().and_then(|()| SetClipboardData(CF_HDROP, Some(HANDLE(block.0))));
        let _ = CloseClipboard();
        if let Err(e) = set {
            let _ = GlobalFree(Some(block));
            return Err(e.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_files_holds_the_header_then_the_wide_path_twice_ended() {
        let data = drop_files(Path::new(r"C:\a.jpg"));
        assert_eq!(&data[..4], &DROPFILES_SIZE.to_le_bytes(), "names start after the header");
        assert_eq!(&data[16..20], &1u32.to_le_bytes(), "the names are UTF-16");
        let names: Vec<u16> = data[20..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let expected: Vec<u16> = r"C:\a.jpg".encode_utf16().chain([0, 0]).collect();
        assert_eq!(names, expected);
    }
}
