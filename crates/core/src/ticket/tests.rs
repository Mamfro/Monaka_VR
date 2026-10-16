use super::*;

fn pose(yaw: f32) -> HeadPose {
    HeadPose::synthetic(yaw, 0.0, HeadPose::SYNTHETIC_FOV, 0.064)
}

#[test]
fn a_frame_is_shown_with_the_pose_it_was_built_from() {
    let mut tickets: Tickets = Tickets::new(4);
    tickets.issue(View::Both, pose(10.0), ());
    tickets.issue(View::Both, pose(20.0), ());
    assert_eq!(tickets.drawing().unwrap().pose, pose(10.0), "the render thread draws the older frame");
    let shown = tickets.show();
    assert_eq!(shown.ticket.unwrap().pose, pose(10.0));
    assert_eq!(shown.waiting, 2);
    assert_eq!(tickets.show().ticket.unwrap().number, 1);
    assert_eq!(tickets.show().ticket, None);
    assert_eq!(tickets.summary(), "3 shown (at most 2 waiting), 1 missing, 0 lost, 0 realigned");
}

#[test]
fn frames_never_shown_are_dropped_oldest_first() {
    let mut tickets: Tickets<u32> = Tickets::new(2);
    for i in 0..5 {
        tickets.issue(View::Eye((i % 2) as usize), pose(0.0), i);
    }
    assert_eq!(tickets.lost(), 3);
    assert_eq!(tickets.drawing().unwrap().extra, 3);
}

#[test]
fn numbered_frames_are_shown_through_their_number() {
    let mut tickets: Tickets = Tickets::new(8);
    for n in [10, 11, 12] {
        tickets.issue_numbered(n, View::Eye((n & 1) as usize), pose(n as f32), ());
    }
    assert_eq!(tickets.find(11).unwrap().view, View::Eye(1));
    let shown = tickets.show_through(11);
    assert_eq!(shown.ticket.unwrap().number, 11);
    assert_eq!(tickets.lost(), 1, "frame 10 went by unshown");
    assert_eq!(tickets.drawing().unwrap().number, 12);
}

/// A pose whose position says which frame it is.
fn frame(n: f32) -> HeadPose {
    HeadPose { position: [n, 0.0, 0.0], valid: 1, ..HeadPose::EMPTY }
}

fn shown_frame(shown: Shown) -> Option<f32> {
    shown.ticket.map(|t| t.pose.position[0])
}

#[test]
fn a_present_behind_drawing_shows_the_frame_before_the_one_drawn() {
    // Frame 1 built, frame 2 built and drawing, then frame 1 presented: the oldest goes.
    let mut tickets = Tickets::<()>::new(4);
    tickets.issue(View::Both, frame(1.0), ());
    tickets.issue(View::Both, frame(2.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(1.0));
    tickets.issue(View::Both, frame(3.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(2.0));
}

#[test]
fn a_queue_a_frame_ahead_takes_the_last_ticket_again_and_then_holds() {
    let mut tickets = Tickets::<()>::new(4);
    // A present before the first frame was drawn takes frame 1's ticket early.
    tickets.issue(View::Both, frame(1.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(1.0));
    // Frame 2 built and drawing, frame 1 presented: only frame 2 waits, so frame 1 again.
    tickets.issue(View::Both, frame(2.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(1.0));
    // From here each present shows the frame before the one drawn.
    tickets.issue(View::Both, frame(3.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(2.0));
    tickets.issue(View::Both, frame(4.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(3.0));
    assert!(tickets.summary().ends_with("1 realigned"), "{}", tickets.summary());
}

#[test]
fn with_nothing_new_built_the_last_ticket_is_taken_again_only_once() {
    let mut tickets = Tickets::<()>::new(4);
    tickets.issue(View::Both, frame(1.0), ());
    tickets.issue(View::Both, frame(2.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(1.0));
    // A menu: presents with nothing new built. Once more frame 1, then the waiting one, then none.
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(1.0));
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(2.0));
    assert_eq!(shown_frame(tickets.show_behind_drawing()), None);
    // Back in play: nothing old is taken again.
    tickets.issue(View::Both, frame(9.0), ());
    assert_eq!(shown_frame(tickets.show_behind_drawing()), Some(9.0));
}
