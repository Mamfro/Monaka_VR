//! The D3D11 redraw's constants against the ones it built itself before they were shared with
//! D3D12: the same sizes and depth test, and a matrix that lands every source pixel where the old
//! one did, as close to true or closer (built around the view's position, so far from the origin
//! it is closer).

use super::*;
use monaka_core::mat4d::{self, Mat4d};

/// The old D3D11 constants of one splat (its own layout, without the output depth's size).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Before {
    m: [f32; 16],
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    depth_width: u32,
    depth_height: u32,
    near_a: f32,
    near_b: f32,
}

fn before(view: &View, color: (u32, u32), depth: (u32, u32), projection: &Mat44, camera: &Mat34) -> Option<Before> {
    let m = reprojection(projection, camera, &view.projection, &view.camera)?;
    Some(Before {
        m,
        width: view.width,
        height: view.height,
        source_width: color.0,
        source_height: color.1,
        depth_width: depth.0,
        depth_height: depth.1,
        near_a: view.mapping.a,
        near_b: view.mapping.b,
    })
}

/// The true matrix, in doubles from the same inputs.
fn truth(view: &View, projection: &Mat44, camera: &Mat34) -> Mat4d {
    let wide = |m: &[f32]| -> Vec<f64> { m.iter().map(|&v| v as f64).collect() };
    let square = |m: &Mat44| -> Mat4d { wide(m).try_into().expect("16") };
    let rigid = |m: &Mat34| -> Mat4d { std::array::from_fn(|i| if i < 12 { m[i] as f64 } else if i == 15 { 1.0 } else { 0.0 }) };
    let target_view = mat4d::inverse(&rigid(&view.camera)).unwrap();
    let unproject = mat4d::inverse(&square(projection)).unwrap();
    mat4d::mul(&mat4d::mul(&mat4d::mul(&square(&view.projection), &target_view), &rigid(camera)), &unproject)
}

/// Where `m` takes a source pixel's normalised coordinates and depth, on screen.
fn land(m: &[f32; 16], ndc: [f32; 3]) -> Option<[f64; 3]> {
    land_true(&m.map(|v| v as f64), ndc)
}

fn land_true(m: &Mat4d, ndc: [f32; 3]) -> Option<[f64; 3]> {
    let v = [ndc[0] as f64, ndc[1] as f64, ndc[2] as f64, 1.0];
    let clip: [f64; 4] = std::array::from_fn(|row| (0..4).map(|k| m[row * 4 + k] * v[k]).sum());
    let p = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    (clip[3] > 0.0 && p[0].abs() <= 1.0 && p[1].abs() <= 1.0).then_some(p)
}

/// How far `p` lands from `t`, in pixels of a frame `width` wide.
fn pixels(p: [f64; 3], t: [f64; 3], width: u32) -> f64 {
    ((p[0] - t[0]).abs() + (p[1] - t[1]).abs()) * width as f64 / 2.0
}

/// A projection with the depth terms of each convention, and its mapping.
fn projections() -> Vec<(Mat44, DepthMapping)> {
    let (near, far) = (0.1f32, 1000.0f32);
    let base = |p10: f32, p11: f32| {
        let mut p = [0.0f32; 16];
        (p[0], p[2], p[5], p[6], p[10], p[11], p[14]) = (0.9, 0.04, 1.6, -0.02, p10, p11, -1.0);
        p
    };
    let standard = base(far / (near - far), near * far / (near - far));
    let reversed = base(near / (far - near), near * far / (far - near));
    let infinite = base(0.0, near);
    [standard, reversed, infinite].into_iter().map(|p| (p, DepthMapping::from_projection(&p, near).unwrap())).collect()
}

/// A camera at `position`, turned `yaw` about up and pitched `pitch`.
fn camera(position: [f32; 3], yaw: f32, pitch: f32) -> Mat34 {
    let (sy, cy, sp, cp) = (yaw.sin(), yaw.cos(), pitch.sin(), pitch.cos());
    let yaw_m = [cy, 0.0, sy, 0.0, 1.0, 0.0, -sy, 0.0, cy];
    let pitch_m = [1.0, 0.0, 0.0, 0.0, cp, -sp, 0.0, sp, cp];
    let r: [f32; 9] = std::array::from_fn(|i| (0..3).map(|k| yaw_m[(i / 3) * 3 + k] * pitch_m[k * 3 + i % 3]).sum());
    [r[0], r[1], r[2], position[0], r[3], r[4], r[5], position[1], r[6], r[7], r[8], position[2]]
}

/// Moved `offset` along its right axis.
fn moved(c: &Mat34, offset: f32) -> Mat34 {
    let mut c = *c;
    (c[3], c[7], c[11]) = (c[3] + offset * c[0], c[7] + offset * c[4], c[11] + offset * c[8]);
    c
}

#[test]
fn every_splat_lands_where_it_did() {
    // Recorded shapes: a 1440x1600 headset eye, a 1920x1080 frame upscaled from a 1280x720 render
    // (its depth at the render size), a small test frame.
    let shapes = [((1440, 1600), (1440, 1600), (1440, 1600)), ((1920, 1080), (1920, 1080), (1280, 720)), ((64, 8), (64, 8), (32, 4))];
    let samples = [[-0.9f32, 0.8], [0.0, 0.0], [0.5, -0.3], [0.95, 0.95]];
    let places = [([0.0, 0.0, 0.0], 0.0, 0.0), ([12.0, 1.7, -30.0], 0.6, -0.2), ([-48.0, 3.1, 25.0], -2.4, 0.4), ([1650.0, 35.0, -820.0], 1.1, 0.1)];
    let mut compared = 0;
    for (projection, mapping) in projections() {
        for (position, yaw, pitch) in places {
            let real = camera(position, yaw, pitch);
            for (offset, (frame, color, depth)) in [-0.064f32, 0.064, 1.0].into_iter().zip(shapes) {
                let view = View { projection, camera: moved(&real, offset), mapping, width: frame.0, height: frame.1 };
                let old = before(&view, color, depth, &projection, &real).unwrap();
                let new = Constants::redraw(&view, None, [0, 0]).splat(&view, color, ([0, 0], depth), [0.0; 2], &projection, &real).unwrap();
                assert_eq!(
                    (new.width, new.height, new.source_width, new.source_height, new.depth_width, new.depth_height, new.near_a, new.near_b),
                    (old.width, old.height, old.source_width, old.source_height, old.depth_width, old.depth_height, old.near_a, old.near_b)
                );
                assert_eq!((new.out_depth_width, new.out_depth_height), (0, 0), "D3D11 writes no depth");
                let truth = truth(&view, &projection, &real);
                for ndc in samples {
                    for nearness in [0.001f32, 0.05, 0.5, 0.99] {
                        let d = (nearness - mapping.b) / mapping.a;
                        let ndc = [ndc[0], ndc[1], d];
                        let (Some(a), Some(b), Some(t)) = (land(&old.m, ndc), land(&new.m, ndc), land_true(&truth, ndc)) else { continue };
                        let (was, is) = (pixels(a, t, frame.0), pixels(b, t, frame.0));
                        // Where it landed, within a tenth of a pixel and at its nearness, near the
                        // origin (farther out the old matrix was off by its own rounding); as close
                        // to true everywhere, to a hundredth of a pixel.
                        if position.iter().all(|v| v.abs() < 100.0) {
                            assert!(pixels(a, b, frame.0) < 0.1, "{offset} {position:?}: {a:?} {b:?}");
                            assert!((mapping.nearness(a[2] as f32) - mapping.nearness(b[2] as f32)).abs() < 1e-4, "{a:?} {b:?}");
                        }
                        assert!(is <= was + 0.01, "{offset} {position:?}: {is} {was}");
                        compared += 1;
                    }
                }
            }
        }
    }
    assert!(compared > 300, "{compared}");
}

#[test]
fn far_from_the_origin_the_shared_matrix_lands_closer_to_true() {
    let (projection, mapping) = projections()[2];
    let real = camera([3000.0, 40.0, -2500.0], 0.6, -0.2);
    let view = View { projection, camera: moved(&real, 0.064), mapping, width: 1440, height: 1600 };
    let new = Constants::redraw(&view, None, [0, 0]).splat(&view, (1440, 1600), ([0, 0], (1440, 1600)), [0.0; 2], &projection, &real).unwrap();
    let old = before(&view, (1440, 1600), (1440, 1600), &projection, &real).unwrap();
    let truth = truth(&view, &projection, &real);
    let ndc = [0.3, -0.2, 0.05];
    let t = land_true(&truth, ndc).unwrap();
    let (was, is) = (pixels(land(&old.m, ndc).unwrap(), t, 1440), pixels(land(&new.m, ndc).unwrap(), t, 1440));
    assert!(is < 0.01, "{is}");
    assert!(is < was, "{is} {was}");
}
