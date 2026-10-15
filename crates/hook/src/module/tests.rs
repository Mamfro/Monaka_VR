use super::*;

#[test]
fn finds_system_modules() {
    let kernel = Module::find("kernel32.dll").expect("kernel32 is loaded");
    let export = kernel.export("GetTickCount64").expect("exported");
    assert!(kernel.contains(export) || Module::containing(export).is_some());
    assert_eq!(Module::containing(kernel.base() + 0x1000), Some(kernel));
    assert!(kernel.file_sha256().is_some_and(|h| h.len() == 64));
    assert!(Module::find("no-such-module.dll").is_none());
    assert!(!host_name().is_empty());
    // The code scanner reads x64 images only (it needs their function table).
    if cfg!(not(target_arch = "x86_64")) {
        return;
    }
    // SAFETY: kernel32 stays loaded for the whole process.
    let code = unsafe { kernel.code() }.expect("kernel32's sections are readable");
    let file = std::fs::read(kernel.path().unwrap()).unwrap();
    let on_disk = Code::from_file(&file).unwrap();
    assert_eq!(code.chunk_count(), on_disk.chunk_count(), "the loaded image lists the same functions as the file");
    let (first, _, _) = code.all_pieces().next().unwrap();
    assert_eq!(code.function(first), Some(first));
}
