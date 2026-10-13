use super::{LogFile, seldom, switched};
use monaka_core::scrub::Scrub;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

#[test]
fn the_first_counts_then_powers_of_two() {
    let logged: Vec<u64> = (1..=40).filter(|&n| seldom(n, 5)).collect();
    assert_eq!(logged, [1, 2, 3, 4, 5, 8, 16, 32]);
}

#[test]
fn with_none_first_only_powers_of_two() {
    let logged: Vec<u64> = (1..=20).filter(|&n| seldom(n, 0)).collect();
    assert_eq!(logged, [1, 2, 4, 8, 16]);
}

/// A fresh log file path for one test.
fn log_path() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("monaka-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}.log", N.fetch_add(1, Relaxed)))
}

/// The text of a log's lines without their time stamps.
fn lines(path: &PathBuf) -> Vec<String> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| line.trim_start().split_once(' ').unwrap().1.to_string()).collect()
}

#[test]
fn lines_wait_in_memory_until_written() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(None)).unwrap();
    log.line(format_args!("one {}", 1));
    log.line(format_args!("two"));
    assert!(lines(&path).is_empty(), "nothing on the disk before the write");
    log.write_pending();
    assert_eq!(lines(&path), ["one 1", "two"]);
    log.line(format_args!("three"));
    log.write_pending();
    log.write_pending();
    assert_eq!(lines(&path), ["one 1", "two", "three"]);
}

#[test]
fn an_urgent_line_writes_the_pending_ones_first() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(None)).unwrap();
    log.line(format_args!("before"));
    log.urgent(format_args!("crash"));
    assert_eq!(lines(&path), ["before", "crash"], "on the disk when urgent returns");
}

#[test]
fn an_urgent_line_gives_up_rather_than_hang_on_a_held_lock() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(None)).unwrap();
    log.line(format_args!("before"));
    let held = log.pending.lock().unwrap();
    log.urgent(format_args!("crash"));
    drop(held);
    assert!(lines(&path).is_empty(), "dropped, not waited for");
    log.write_pending();
    assert_eq!(lines(&path), ["before"]);
}

#[test]
fn lines_from_many_threads_all_arrive_whole() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(None)).unwrap();
    std::thread::scope(|scope| {
        for thread in 0..4 {
            let log = &log;
            scope.spawn(move || {
                for n in 0..500 {
                    log.line(format_args!("t{thread} n{n}"));
                    if n % 50 == 0 {
                        log.write_pending();
                    }
                }
            });
        }
    });
    log.write_pending();
    let written = lines(&path);
    assert_eq!(written.len(), 2000);
    for thread in 0..4 {
        let ns: Vec<u32> = written.iter().filter_map(|line| line.strip_prefix(&format!("t{thread} n"))).map(|n| n.parse().unwrap()).collect();
        assert_eq!(ns, (0..500).collect::<Vec<_>>(), "thread {thread}'s lines in order");
    }
}

#[test]
fn the_players_folder_is_taken_out_as_lines_are_written() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(Some(r"D:\Home\Bob"))).unwrap();
    log.line(format_args!(r"options from D:\Home\Bob\AppData\Local\monaka\runs\x"));
    log.urgent(format_args!(r"exception in D:\Home\Bob\game.dll"));
    assert_eq!(lines(&path), [r"options from %USERPROFILE%\AppData\Local\monaka\runs\x", r"exception in %USERPROFILE%\game.dll"]);
}

#[test]
fn detailed_lines_follow_the_setting_until_the_log_is_full() {
    assert_eq!(switched(false, true, false), Some((true, "on")));
    assert_eq!(switched(true, true, false), None, "no change, no line");
    assert_eq!(switched(true, false, false), Some((false, "off")));
    assert_eq!(switched(true, true, true), Some((false, "stopped: the log reached its size limit")));
    assert_eq!(switched(false, true, true), None, "a full log stays off");
}

#[test]
fn the_log_counts_what_it_wrote() {
    let path = log_path();
    let log = LogFile::create(&path, Scrub::for_profile(None)).unwrap();
    log.line(format_args!("abc"));
    log.write_pending();
    assert_eq!(log.written(), std::fs::metadata(&path).unwrap().len());
}
