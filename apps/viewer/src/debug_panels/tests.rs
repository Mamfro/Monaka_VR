use super::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

fn ids(panels: &HeadsetPanels) -> Vec<PanelId> {
    panels.shown.iter().map(|s| s.id).collect()
}

#[test]
fn the_panels_follow_the_tab_in_its_order_keeping_what_stays() {
    let mut panels = HeadsetPanels::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    panels.follow(Panels::NONE.with(PanelId::Console, true));
    panels.shown[0].drawn_at = Some(Instant::now());
    panels.follow(Panels::NONE.with(PanelId::Console, true).with(PanelId::Stats, true));
    assert_eq!(ids(&panels), Panels::NONE.with(PanelId::Console, true).with(PanelId::Stats, true).iter().collect::<Vec<_>>(), "the tab's order");
    let console = panels.shown.iter().find(|s| s.id == PanelId::Console).unwrap();
    assert!(console.drawn_at.is_some(), "the console kept, not made again");
    panels.follow(Panels::NONE.with(PanelId::Stats, true));
    assert_eq!(ids(&panels), [PanelId::Stats]);
    assert_eq!(panels.len(), 1);
}
