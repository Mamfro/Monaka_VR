use super::Scrub;

fn bob() -> Scrub {
    Scrub::for_profile(Some(r"D:\Home\Bob"))
}

#[test]
fn the_profile_folder_becomes_the_placeholder() {
    assert_eq!(bob().text(r"opened D:\Home\Bob\AppData\Local\monaka\runs\x\dl1.log"), r"opened %USERPROFILE%\AppData\Local\monaka\runs\x\dl1.log");
    assert_eq!(bob().text(r"d:/home/bob/Documents and D:\HOME\BOB"), r"%USERPROFILE%/Documents and %USERPROFILE%");
    assert_eq!(bob().text(r#"path "D:\Home\Bob" twice D:\Home\Bob\x"#), r#"path "%USERPROFILE%" twice %USERPROFILE%\x"#);
}

#[test]
fn a_longer_name_or_other_text_is_left_alone() {
    assert_eq!(bob().text(r"D:\Home\Bobby\x and D:\Games\Bob"), r"D:\Home\Bobby\x and D:\Games\Bob");
    assert_eq!(bob().text("no paths here, Ünïcode kept"), "no paths here, Ünïcode kept");
}

#[test]
fn no_profile_or_a_bare_drive_scrubs_nothing() {
    for scrub in [Scrub::for_profile(None), Scrub::for_profile(Some(r"D:\"))] {
        assert_eq!(scrub.text(r"D:\Home\Bob\x"), r"D:\Home\Bob\x");
    }
    assert_eq!(Scrub::for_profile(Some(r"D:\Home\Bob\")).text(r"D:\Home\Bob\x"), r"%USERPROFILE%\x");
}
