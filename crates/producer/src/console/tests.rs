use super::*;

fn base(module: &str) -> Option<usize> {
    match module {
        "exe" => Some(0x1400_0000),
        "d3d11.dll" => Some(0x7ff0_0000),
        _ => None,
    }
}

#[test]
fn addresses_are_read_in_each_form() {
    assert_eq!(parse_address("exe+0x25f930", base), Ok(0x1400_0000 + 0x25f930));
    assert_eq!(parse_address("d3d11.dll+16", base), Ok(0x7ff0_0010));
    assert_eq!(parse_address("0x7ff612340000", base), Ok(0x7ff6_1234_0000));
    assert!(parse_address("game.dll+0x10", base).unwrap_err().contains("no such module"));
    assert!(parse_address("exe+zz", base).is_err());
}

#[cfg(target_arch = "x86_64")]
#[test]
fn snapshots_are_read_with_their_offsets() {
    use monaka_hook::live::{Register, Snapshot};
    assert_eq!(parse_snapshot("rcx:0x200"), Ok(Snapshot { register: Register::Rcx, offset: 0, length: 0x200 }));
    assert_eq!(parse_snapshot("this+0x1860:64"), Ok(Snapshot { register: Register::Rcx, offset: 0x1860, length: 64 }));
    assert_eq!(parse_snapshot("rdx-8:16"), Ok(Snapshot { register: Register::Rdx, offset: -8, length: 16 }));
    assert!(parse_snapshot("rax:16").is_err());
}

#[test]
fn commands_answer_in_text() {
    assert!(run("help").contains("hook <addr>"));
    assert!(run("").contains("timeline"));
    assert!(run("nonsense").starts_with("error: unknown command"));
    assert!(run("read").starts_with("error: which address"));
    // Our own memory reads back.
    let value: u32 = 0x1234_5678;
    let answer = run(&format!("read {:#x} 4", &value as *const u32 as usize));
    assert!(answer.contains("78 56 34 12") && answer.contains("305419896"), "{answer}");
}

/// A tool's question crosses the channel, the run's timer answers it, the tool reads the answer.
#[test]
fn a_question_crosses_the_channel() {
    let name = monaka_channel::ChannelName::new(format!(r"Local\monaka-test-console-{}", std::process::id())).expect("name");
    *CHANNEL.locked() = Some(ConsoleChannel::open(&name).expect("producer side"));
    let mut tool = ConsoleChannel::open(&name).expect("tool side");
    let number = tool.ask("help").expect("asked");
    tick(0.05);
    assert!(tool.answer_to(number).expect("answered").contains("hook <addr>"));
    *CHANNEL.locked() = None;
}

#[test]
fn finding_commands_answer_from_a_loaded_module() {
    let answer = run("strings kernel32.dll kernelbase.dll");
    assert!(answer.to_ascii_lowercase().contains("used by kernelbase.dll+0x"), "{answer}");
    let values = [0u32, 7, 0x3fc0_0000, 7];
    let answer = run(&format!("find {:#x} 16 1.5", values.as_ptr() as usize));
    assert!(answer.starts_with("1 places") && answer.contains("+0x8"), "{answer}");
    let answer = run(&format!("find {:#x} 16 7", values.as_ptr() as usize));
    assert!(answer.starts_with("2 places"), "{answer}");
}

#[cfg(target_arch = "x86_64")]
#[test]
fn a_hook_inside_a_function_is_refused_without_force() {
    let inside = run as fn(&str) -> String as usize + 1;
    let answer = run(&format!("hook {inside:#x}"));
    assert!(answer.contains("is inside the function at"), "{answer}");
}

#[cfg(target_arch = "x86_64")]
#[test]
fn a_search_needs_its_start_first() {
    assert!(run("cover action").contains("press Start first"));
    assert!(run("cover inaction").contains("press Start first"));
    assert!(run("cover candidates").contains("no search"));
    assert!(run("cover undo").contains("press Start first"));
}
