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
    assert!(!references.is_empty(), "kernelbase uses the name it holds");
    for &(target, site) in &references {
        assert!(strings.contains(&target));
        assert!(code.function(site).is_some(), "every confirmed reference is inside a function");
    }
}

#[test]
fn texts_holding_a_word_and_their_users_are_found() {
    let folder = if cfg!(target_arch = "x86") { "Sysnative" } else { "System32" };
    let bytes = std::fs::read(format!(r"C:\Windows\{folder}\kernelbase.dll")).expect("kernelbase.dll is readable");
    let code = Code::from_file(&bytes).unwrap();
    let texts = code.texts_containing("KERNEL32", 50);
    assert!(texts.iter().any(|t| t.wide && t.text.to_ascii_lowercase().ends_with("kernel32.dll") && t.text.len() < 16), "{texts:?}");
    let users = code.users(&texts);
    assert!(!users.is_empty());
    assert!(users.iter().all(|(target, function, _)| texts.iter().any(|t| t.rva == *target) && code.bytes(*function, 1).is_some()));
}

#[test]
fn classes_are_found_by_name_with_their_methods() {
    let folder = if cfg!(target_arch = "x86") { "Sysnative" } else { "System32" };
    let bytes = std::fs::read(format!(r"C:\Windows\{folder}\msvcp140.dll")).expect("msvcp140.dll is readable");
    let code = Code::from_file(&bytes).unwrap();
    let classes = code.classes("exception", 20);
    let base = classes.iter().find(|c| c.name == "std::exception").expect("std::exception is named");
    assert!(!base.tables.is_empty(), "its method table is found");
    assert!(base.tables.iter().all(|t| !t.methods.is_empty() && t.methods.iter().all(|m| code.is_executable(*m))));
}

#[test]
fn plain_rtti_names_read_as_scoped_names() {
    assert_eq!(demangle(".?AVarkSkinnedRenderModel@@"), "arkSkinnedRenderModel");
    assert_eq!(demangle(".?AVexception@std@@"), "std::exception");
    assert_eq!(demangle(".?AV?$basic_ios@DU?$char_traits@D@std@@@std@@"), ".?AV?$basic_ios@DU?$char_traits@D@std@@@std@@");
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
