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
    assert_eq!(tickets.summary(), "3 shown (at most 2 waiting), 1 missing, 0 lost");
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
