use super::*;

#[test]
fn a_stack_line_cuts_at_a_character() {
    let mut line = StackLine::new();
    let _ = write!(line, "{}", "a".repeat(1022));
    // "é" is two bytes: it fits once, and the next one is cut whole, not in half.
    let _ = line.write_str("éé");
    assert_eq!(line.as_str().len(), 1024);
    assert!(line.as_str().ends_with('é'));
    let _ = line.write_str("more");
    assert_eq!(line.as_str().len(), 1024, "a full line takes nothing more");
}

#[test]
fn an_address_is_described_by_its_module() {
    let mut line = StackLine::new();
    describe_into(&mut line, a_stack_line_cuts_at_a_character as fn() as usize);
    let text = line.as_str();
    assert!(text.contains(".exe+0x") || text.contains(".dll+0x"), "{text}");
    let mut line = StackLine::new();
    describe_into(&mut line, 0x10);
    assert_eq!(line.as_str(), "0x10 (no module)");
}
