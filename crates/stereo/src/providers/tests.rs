use super::*;

fn first(_: &ID3D12CommandQueue, _: &ID3D12Resource) {}
fn second(_: &ID3D12CommandQueue, _: &ID3D12Resource) {}
fn beside(_: &ID3D12CommandQueue, _: &ID3D12Resource, _: [Params; 2]) {}

#[test]
fn a_feature_joins_the_hud_once_and_leaves_it() {
    join_hud(first, beside);
    join_hud(first, beside);
    join_hud(second, beside);
    assert_eq!(BEFORE.locked().len(), 2);
    assert_eq!(BESIDE.locked().len(), 1);
    leave_hud(first, beside);
    assert!(BEFORE.locked().iter().all(|&b| std::ptr::fn_addr_eq(b, second as BeforeHud)));
    assert!(BESIDE.locked().is_empty());
    leave_hud(second, beside);
}
