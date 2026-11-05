use super::files::*;

const N: u32 = 8;

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / l)
}

/// The strip pixel at face `face`, column `i`, row `j`.
fn at(face: usize, i: usize, j: usize) -> usize {
    j * 6 * N as usize + face * N as usize + i
}

#[test]
fn every_pixel_middle_points_back_to_its_pixel() {
    for face in 0..6 {
        for j in 0..N as usize {
            for i in 0..N as usize {
                let dir = direction(N, face, i as f32 + 0.5, j as f32 + 0.5);
                assert_eq!(strip_pixel(N, dir), at(face, i, j), "face {face} ({i}, {j})");
            }
        }
    }
}

#[test]
fn faces_look_where_the_capture_says() {
    let middle = |face| unit(direction(N, face, N as f32 / 2.0, N as f32 / 2.0));
    assert_eq!(middle(0), [0.0, 0.0, -1.0], "front is forward (-z)");
    assert_eq!(middle(1), [1.0, 0.0, 0.0], "right");
    assert_eq!(middle(2), [0.0, 0.0, 1.0], "back");
    assert_eq!(middle(3), [-1.0, 0.0, 0.0], "left");
    assert_eq!(middle(4), [0.0, 1.0, 0.0], "up");
    assert_eq!(middle(5), [0.0, -1.0, 0.0], "down");
    // The up face's bottom edge is the front face's top edge, and the down face's top edge its bottom.
    let n = N as f32;
    assert_eq!(unit(direction(N, 4, n / 2.0, n)), unit(direction(N, 0, n / 2.0, 0.0)));
    assert_eq!(unit(direction(N, 5, n / 2.0, 0.0)), unit(direction(N, 0, n / 2.0, n)));
    // The right face's left edge is the front face's right edge.
    assert_eq!(unit(direction(N, 1, 0.0, n / 2.0)), unit(direction(N, 0, n, n / 2.0)));
}

#[test]
fn a_flat_room_has_no_cracks() {
    // Everything 3 m away: every corner is 3 m, so neighbouring tiles meet, across faces too.
    let depth = vec![3.0; (N * N * 6) as usize];
    assert!(corners(&depth, N).iter().flatten().all(|&d| (d - 3.0).abs() < 1e-6));
}

#[test]
fn a_tile_ends_where_its_pixel_does_at_an_edge() {
    // A near thing in the middle of the front face, the wall behind it.
    let mut depth = vec![4.0; (N * N * 6) as usize];
    let near = at(0, 4, 4);
    depth[near] = 1.0;
    let made = corners(&depth, N);
    assert_eq!(made[near], [1.0; 4], "the near tile keeps its own distance");
    // The wall beside it is not pulled forward.
    assert_eq!(made[at(0, 3, 4)], [4.0; 4]);
    // A gentle slope is one surface: its corners are the mean.
    let mut sloped = vec![2.0; (N * N * 6) as usize];
    sloped[at(0, 4, 4)] = 2.1;
    assert!((corners(&sloped, N)[at(0, 3, 4)][1] - (2.0 * 3.0 + 2.1) / 4.0).abs() < 1e-5);
}

#[test]
fn another_capture_fills_only_what_the_middle_did_not_see() {
    // A wall 4 m ahead and a post 1 m ahead in the middle of the front face.
    let n = 16u32;
    let room = |offset: [f32; 3]| -> Vec<f32> {
        (0..(n * n * 6) as usize)
            .map(|pixel| {
                let width = 6 * n as usize;
                let (x, y) = (pixel % width, pixel / width);
                let dir = unit(direction(n, x / n as usize, (x % n as usize) as f32 + 0.5, y as f32 + 0.5));
                // The post: a 0.2 m square facing the capture, 1 m ahead; then the far box 4 m away.
                let to_post = (-1.0 - offset[2]) / dir[2];
                let hit = [0, 1, 2].map(|k| offset[k] + dir[k] * to_post);
                if dir[2] < 0.0 && hit[0].abs() < 0.1 && hit[1].abs() < 0.1 {
                    return to_post;
                }
                (0..3).map(|k| if dir[k] == 0.0 { f32::MAX } else { ((if dir[k] > 0.0 { 4.0 } else { -4.0 }) - offset[k]) / dir[k] }).fold(f32::MAX, f32::min)
            })
            .collect()
    };
    let centre = room([0.0; 3]);
    let right = room([0.5, 0.0, 0.0]);
    let kept = unseen(&centre, &right, [0.5, 0.0, 0.0], n);
    assert!(!kept.is_empty(), "the wall behind the post is new from the right");
    assert!(kept.len() < (n * n * 6) as usize / 20, "most of what the right sees, the middle saw: {} kept", kept.len());
    // Every pixel kept is on the front face, near the post.
    for pixel in &kept {
        let x = *pixel as usize % (6 * n as usize);
        assert_eq!(x / n as usize, 0, "pixel {pixel} is not on the front face");
    }
}

/// Times as people write them, in minutes.
fn at_clock(hour: u16, minute: u16) -> Minutes {
    hour * 60 + minute
}

/// The capture the Kitchen has: midnight, then every half hour from 5:00 to 20:00.
fn half_hours() -> Vec<Minutes> {
    std::iter::once(0).chain((10..=40).map(|k| k * 30)).collect()
}

#[test]
fn image_names_say_their_time() {
    assert_eq!(time_of("hour_05.png"), Some(at_clock(5, 0)));
    assert_eq!(time_of("hour_19_30.png"), Some(at_clock(19, 30)));
    assert_eq!(time_of("hour_24.png"), None);
    assert_eq!(time_of("depth.exr"), None);
    let folder = std::path::Path::new("room");
    assert!(colour_path(folder, "c", at_clock(7, 30)).ends_with("c/hour_07_30.png"));
    assert!(colour_path(folder, "c", at_clock(7, 0)).ends_with("c/hour_07.png"));
    assert_eq!(clock_text(at_clock(7, 5)), "7:05");
}

#[test]
fn each_capture_fades_into_the_next_as_it_comes() {
    let times = half_hours();
    let seconds = |hour: f64, minute: f64| hour * 3600.0 + minute * 60.0;
    assert_eq!(shown_times(&times, seconds(15.0, 10.0)), (at_clock(15, 0), at_clock(15, 30), 0.0));
    let (now, next, fade) = shown_times(&times, seconds(15.0, 30.0) - 5.0);
    assert_eq!((now, next), (at_clock(15, 0), at_clock(15, 30)));
    assert!((fade - 0.5).abs() < 1e-3, "half way through the fade: {fade}");
    // The night: the last capture of the evening until midnight's, then midnight's until morning.
    assert_eq!(shown_times(&times, seconds(22.0, 0.0)).0, at_clock(20, 0));
    assert_eq!(shown_times(&times, seconds(3.0, 0.0)), (0, at_clock(5, 0), 0.0));
    assert_eq!(shown_times(&[at_clock(12, 0)], seconds(3.0, 0.0)), (at_clock(12, 0), at_clock(12, 0), 0.0));
}

#[test]
fn swept_times_blend_all_the_way_through() {
    let times = half_hours();
    let (now, next, far) = blended_times(&times, 15.0 * 3600.0 + 7.5 * 60.0);
    assert_eq!((now, next), (at_clock(15, 0), at_clock(15, 30)));
    assert!((far - 0.25).abs() < 1e-4);
    assert_eq!(blended_times(&times, 15.0 * 3600.0), (at_clock(15, 0), at_clock(15, 30), 0.0), "on a capture: it alone");
    // Round the night's long gap and midnight.
    let (now, next, far) = blended_times(&times, 22.0 * 3600.0);
    assert_eq!((now, next), (at_clock(20, 0), 0));
    assert!((far - 0.5).abs() < 1e-4);
}

#[test]
fn letting_go_lands_on_the_nearest_capture() {
    let times = half_hours();
    assert_eq!(nearest_time(&times, 15.0 * 3600.0 + 20.0 * 60.0), 15.5 * 3600.0);
    assert_eq!(nearest_time(&times, 15.0 * 3600.0 + 10.0 * 60.0), 15.0 * 3600.0);
    // Just before midnight: midnight, as tomorrow.
    assert_eq!(nearest_time(&times, 23.9 * 3600.0), 24.0 * 3600.0);
}
