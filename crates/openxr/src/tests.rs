use super::*;

#[test]
fn names_are_truncated_and_terminated() {
    let mut field = [0 as c_char; 4];
    copy_name(&mut field, "abcdef");
    assert_eq!(text(&field), "abc");
    assert_eq!(field[3], 0);
}

#[test]
fn loader_requires_an_absolute_path() {
    assert!(matches!(Loader::load(Path::new("openxr_loader.dll")), Err(Error::Loader(_))));
}
