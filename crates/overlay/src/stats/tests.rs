use super::*;

#[test]
fn a_rate_and_time_reads_as_one_line() {
    assert_eq!(rate_and_time(71.94, 13.9, 22.06), "71.9 / s   13.9 ms (longest 22.1)");
}

#[test]
fn a_graph_spans_twice_the_budget_or_its_longest_frame() {
    assert_eq!(graph_top(&[10.0, 12.0], 11.1), 22.2, "two budgets");
    assert_eq!(graph_top(&[10.0, 40.0], 11.1), 40.0, "a hitch raises the top");
    assert_eq!(graph_top(&[10.0, 400.0], 11.1), 100.0, "a stall does not flatten the rest");
    assert_eq!(graph_height(11.1, 22.2), 0.5);
    assert_eq!(graph_height(400.0, 100.0), 1.0, "past the top: at the top");
}
