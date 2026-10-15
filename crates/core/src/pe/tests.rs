use super::*;

#[test]
fn host_lists_name_one_or_several_executables() {
    assert!(host_listed("DyingLightGame.exe", "dyinglightgame.exe"));
    assert!(host_listed("WolfNewOrder_x64.exe;WolfOldBlood_x64.exe", "WolfOldBlood_x64.exe"));
    assert!(!host_listed("WolfNewOrder_x64.exe;WolfOldBlood_x64.exe", "Wolf"));
    assert!(!host_listed("a.exe;", ""));
    assert!(host_listed("*", "AnyGame.exe") && !host_listed("*", ""));
}

#[test]
fn finds_a_32_bit_system_export() {
    let path = std::path::Path::new(r"C:\Windows\SysWOW64\kernel32.dll");
    let bytes = std::fs::read(path).expect("the 32-bit kernel32.dll is readable");
    assert!(file_export(&bytes, "LoadLibraryW").unwrap().is_some_and(|rva| rva > 0x1000));
}

#[test]
fn finds_a_system_export() {
    let path = std::path::Path::new(r"C:\Windows\System32\kernelbase.dll");
    let bytes = std::fs::read(path).expect("kernelbase.dll is readable");
    let rva = file_export(&bytes, "GetTickCount64").unwrap().expect("exported");
    assert!(rva > 0x1000);
    assert_eq!(file_export(&bytes, "GetTickCount6").unwrap(), None, "a prefix is not a match");
    assert_eq!(file_export(&bytes, "NoSuchExport").unwrap(), None);
    assert!(file_export(b"not a pe", "x").is_err());
}
