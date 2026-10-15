use super::*;

#[test]
fn reads_a_swapchains_own_queue() {
    learn().expect("offset learned");
    let queue = direct_queue().unwrap();
    let chain = composition_chain(&queue).unwrap();
    // SAFETY: our own live swapchain.
    let found = unsafe { of(chain.as_raw()) }.expect("queue read back");
    assert_eq!(found.as_raw(), queue.as_raw(), "the second swapchain's queue, not the first's");
}
