use super::*;

#[test]
fn the_slots_are_d3d12s() {
    assert_eq!(slot(offset_of!(ID3D12CommandQueue_Vtbl, ExecuteCommandLists)), 10);
    assert_eq!(slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, DrawInstanced)), 12);
    assert_eq!(slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, DrawIndexedInstanced)), 13);
    assert_eq!(slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, OMSetRenderTargets)), 46);
    assert_eq!(slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, BeginEvent)), 57);
}

static FIRST: Subscriber = Subscriber { name: "first", ..Subscriber::NONE };
static SECOND: Subscriber = Subscriber { name: "second", ..Subscriber::NONE };

#[test]
fn a_subscriber_leaves_once_and_the_others_stay_in_order() {
    for subscriber in [&FIRST, &SECOND] {
        let free = SUBSCRIBED.iter().find(|p| p.load(Acquire).is_null()).expect("a free place");
        free.store(pointer(subscriber), Release);
    }
    assert_eq!(subscribers().map(|s| s.name).collect::<Vec<_>>(), ["first", "second"]);
    leave(&FIRST, 0);
    leave(&FIRST, 0);
    assert_eq!(subscribers().map(|s| s.name).collect::<Vec<_>>(), ["second"]);
    leave(&SECOND, 0);
    assert_eq!(subscribers().count(), 0);
    assert!(!hooked());
}
