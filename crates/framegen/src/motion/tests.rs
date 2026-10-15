use super::*;

#[test]
fn the_shaders_compile_and_their_constants_fit() {
    compute::compile(SOURCE, "motion_fix.hlsl", s!("Fix")).expect("motion_fix.hlsl compiles");
    compute::compile(COMPOSE_SOURCE, "motion_compose.hlsl", s!("Compose")).expect("motion_compose.hlsl compiles");
    compute::compile(WARP_SOURCE, "motion_warp.hlsl", s!("Warp")).expect("motion_warp.hlsl compiles");
    assert_eq!(size_of::<Constants>(), 56 * 4);
}
