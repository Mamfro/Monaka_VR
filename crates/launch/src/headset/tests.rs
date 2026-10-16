use super::*;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("monaka-headset-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_file_is_written_whole_over_the_last() {
    let dir = folder("write");
    write(&dir, "a.txt", "one").unwrap();
    write(&dir, "a.txt", "two").unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "two");
    assert!(!dir.join("a.txt.part").exists(), "nothing left beside it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_takes_the_eye_size_the_view_left() {
    let dir = folder("eye");
    let (_send, lines) = mpsc::channel();
    let linked = Linked { folder: dir.clone(), lines, pid: 1 };
    assert_eq!(linked.eye_size(), None, "not said yet");
    std::fs::write(dir.join(home::EYE_SIZE_FILE), home::eye_size_text((2064, 2208))).unwrap();
    assert_eq!(linked.eye_size(), Some((2064, 2208)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_links_its_game_in_and_lets_it_go() {
    let dir = folder("link");
    let (_send, lines) = mpsc::channel();
    let linked = Linked { folder: dir.clone(), lines, pid: 1 };
    let args = ["--option".to_owned(), "dpad_menus=left".to_owned()];
    linked.show_game(r"Local\monaka-1", &args).unwrap();
    let read = || home::parse_link(&std::fs::read_to_string(dir.join(home::LINK_FILE)).unwrap());
    assert_eq!(read(), Some((r"Local\monaka-1".to_owned(), args.to_vec())));
    linked.let_go();
    assert_eq!(read(), None);
    let _ = std::fs::remove_dir_all(&dir);
}
