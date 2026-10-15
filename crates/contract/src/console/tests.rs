use super::*;

fn blank() -> Box<ConsoleBlock> {
    let mut block = Box::new(ConsoleBlock {
        stamp: Stamp::default(),
        asked: 0,
        answered: 0,
        command_len: 0,
        answer_len: 0,
        command: [0; COMMAND_MAX],
        answer: [0; ANSWER_MAX],
        generation: 0,
        status_len: 0,
        log_len: 0,
        status: [0; STATUS_MAX],
        log: [0; LOG_MAX],
    });
    block.start();
    block
}

#[test]
fn a_question_is_answered_once() {
    let mut block = blank();
    assert!(block.pending().is_none());
    let number = block.ask("hook exe+0x1234");
    assert_eq!(block.pending(), Some((number, "hook exe+0x1234".to_owned())));
    assert!(block.answer_to(number).is_none(), "not answered yet");
    block.answer(number, "probe 0 at exe+0x1234");
    assert!(block.pending().is_none());
    assert_eq!(block.answer_to(number).as_deref(), Some("probe 0 at exe+0x1234"));
}

#[test]
fn a_newer_question_replaces_one_not_taken() {
    let mut block = blank();
    block.ask("list");
    let newer = block.ask("help");
    assert_eq!(block.pending(), Some((newer, "help".to_owned())));
}

#[test]
fn long_text_is_cut_at_a_character_and_says_so() {
    let mut block = blank();
    let number = block.ask(&"日".repeat(COMMAND_MAX));
    let (_, command) = block.pending().expect("pending");
    assert_eq!(command.len(), COMMAND_MAX - COMMAND_MAX % 3);
    block.answer(number, &"x".repeat(ANSWER_MAX + 10));
    let answer = block.answer_to(number).expect("answered");
    assert!(answer.len() <= ANSWER_MAX && answer.ends_with("holds)"));
}

#[test]
fn the_log_keeps_the_newest_exchanges_and_the_status_its_last() {
    let mut block = blank();
    block.set_status("probes: 0");
    block.set_status("probes: 1");
    assert_eq!(block.status_text(), "probes: 1");
    block.post("list", "0 arm at exe+0x25f930\n    from exe+0x341220");
    assert_eq!(block.log_text(), "> list\n0 arm at exe+0x25f930\n    from exe+0x341220\n");
    let long: String = (0..100).map(|k| format!("line {k}\n")).collect();
    block.post("calls 0", &long);
    assert!(block.log_text().ends_with("line 23\n  (76 more lines)\n"));
    for k in 0..2000 {
        block.post(&format!("where {k}"), "somewhere");
    }
    let log = block.log_text();
    assert!(log.len() <= LOG_MAX && log.starts_with("> where ") && log.ends_with("> where 1999\nsomewhere\n"), "whole lines, the newest kept");
}
