//! Light or dark, as Windows' app mode says (Settings > Personalisation > Colours), switching live
//! when it changes. Light is the system colours, as the window always had. Plain Win32 controls only
//! partly darken, so each kind gets its own fix:
//!
//! - the title bar: DWM's immersive dark mode;
//! - the window, labels, the log, edits and lists: our colours from the `WM_CTLCOLOR*` messages;
//! - buttons and scrollbars: the `DarkMode_Explorer` theme, drop-downs `DarkMode_CFD` (the names
//!   Explorer and Notepad++ use; not documented, but stable since Windows 10 1809);
//! - checkboxes: the theme darkens the box but ignores the text colour, so the box and label are
//!   drawn here on `NM_CUSTOMDRAW`;
//! - the option tabs: no theme darkens a tab control, so a subclass paints the strip.
//!
//! High contrast keeps the system colours.

use crate::send;
use std::cell::Cell;
use std::sync::OnceLock;
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateSolidBrush, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DRAW_TEXT_FORMAT, DrawFocusRect, DrawTextW, EndPaint, FillRect, GetSysColor, HBRUSH, HDC, HGDIOBJ, PAINTSTRUCT, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME,
    RDW_INVALIDATE, RedrawWindow, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Controls::{
    BP_CHECKBOX, CBS_CHECKEDNORMAL, CBS_UNCHECKEDNORMAL, CDDS_PREPAINT, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED, CDRF_SKIPDEFAULT, COMBOBOXINFO,
    CloseThemeData, DrawThemeBackground, GetComboBoxInfo, GetThemePartSize, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, SetWindowTheme, TCIF_TEXT, TCITEMW,
    TCM_GETCURSEL, TCM_GETITEMCOUNT, TCM_GETITEMRECT, TCM_GETITEMW, TS_DRAW,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, OpenThemeDataForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BS_AUTOCHECKBOX, BS_TYPEMASK, EnumChildWindows, GWL_STYLE, GetClassNameW, GetClientRect, GetWindowLongW, GetWindowTextW, SPI_GETHIGHCONTRAST,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, UISF_HIDEFOCUS, WM_ERASEBKGND, WM_GETFONT, WM_PAINT, WM_QUERYUISTATE,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

/// One set of colours, and brushes of them.
pub struct Theme {
    pub dark: bool,
    pub text: COLORREF,
    /// Help lines, the status meaning, unselected tabs.
    pub grey: COLORREF,
    pub background: COLORREF,
    pub background_brush: HBRUSH,
    /// Inside boxes: the game list, the log, edits.
    pub surface: COLORREF,
    pub surface_brush: HBRUSH,
    /// The tab strip's edges.
    line_brush: HBRUSH,
}

// SAFETY: the brushes are GDI handles, used only on the window's thread; the statics only hold them.
unsafe impl Send for Theme {}
unsafe impl Sync for Theme {}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

impl Theme {
    fn new(dark: bool) -> Theme {
        // SAFETY: system colours, and brushes kept for the life of the process (controls may hold
        // the one they were handed between paints, so they are never freed).
        unsafe {
            let (text, grey, background, surface, line) = if dark {
                (rgb(240, 240, 240), rgb(160, 160, 160), rgb(32, 32, 32), rgb(43, 43, 43), rgb(77, 77, 77))
            } else {
                let window = COLORREF(GetSysColor(COLOR_WINDOW));
                (COLORREF(GetSysColor(COLOR_WINDOWTEXT)), COLORREF(GetSysColor(COLOR_GRAYTEXT)), window, window, COLORREF(GetSysColor(COLOR_GRAYTEXT)))
            };
            Theme {
                dark,
                text,
                grey,
                background,
                background_brush: CreateSolidBrush(background),
                surface,
                surface_brush: CreateSolidBrush(surface),
                line_brush: CreateSolidBrush(line),
            }
        }
    }
}

static LIGHT: OnceLock<Theme> = OnceLock::new();
static DARK: OnceLock<Theme> = OnceLock::new();

thread_local! {
    static IS_DARK: Cell<bool> = const { Cell::new(false) };
    /// `--theme` on the command line: this one whatever Windows says.
    static FORCED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// The colours in use now.
pub fn current() -> &'static Theme {
    if IS_DARK.get() { DARK.get_or_init(|| Theme::new(true)) } else { LIGHT.get_or_init(|| Theme::new(false)) }
}

/// Picks the theme before the window exists; `forced` overrides Windows' app mode.
pub fn start(forced: Option<bool>) {
    FORCED.set(forced);
    IS_DARK.set(wanted());
}

/// Dark when Windows' apps are dark and high contrast is off.
fn wanted() -> bool {
    if let Some(forced) = FORCED.get() {
        return forced;
    }
    let mut contrast = HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    // SAFETY: fills our HIGHCONTRASTW of the size it says.
    let read = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, contrast.cbSize, Some((&raw mut contrast).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    if read.is_ok() && contrast.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return false;
    }
    let mut light = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into ours.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&mut size),
        )
    };
    result == ERROR_SUCCESS && light == 0
}

/// The title bar to match.
pub fn window(hwnd: HWND) {
    let dark = BOOL::from(current().dark);
    // SAFETY: sets an attribute of our own window from a BOOL of the size given; older Windows
    // without it keep a light title bar.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const dark).cast(), size_of::<BOOL>() as u32) };
}

/// Themes every child control of `parent` (call again after adding controls).
pub fn controls(parent: HWND) {
    // SAFETY: walks our own window's children with a callback that only themes them.
    unsafe {
        let _ = EnumChildWindows(Some(parent), Some(theme_child), LPARAM(0));
    }
}

extern "system" fn theme_child(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut class = [0u16; 64];
    // SAFETY: reads a child's class name into our buffer.
    let length = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    let class = String::from_utf16_lossy(&class[..length]);
    let dark = current().dark;
    // Light hands the control back to its default theme.
    let set = |hwnd: HWND, name: PCWSTR| {
        // SAFETY: themes one of our controls with a static name, or the default with null.
        let _ = unsafe { SetWindowTheme(hwnd, if dark { name } else { PCWSTR::null() }, PCWSTR::null()) };
    };
    match class.as_str() {
        "Button" | "ListBox" | "Edit" => set(hwnd, w!("DarkMode_Explorer")),
        "ComboBox" => {
            set(hwnd, w!("DarkMode_CFD"));
            let mut info = COMBOBOXINFO { cbSize: size_of::<COMBOBOXINFO>() as u32, ..Default::default() };
            // SAFETY: the drop-down's parts, into our COMBOBOXINFO of the size it says.
            if unsafe { GetComboBoxInfo(hwnd, &mut info) }.is_ok() {
                set(info.hwndList, w!("DarkMode_Explorer"));
            }
        }
        "SysTabControl32" => {
            // SAFETY: subclasses our tab control; installing it again only replaces it.
            let _ = unsafe { SetWindowSubclass(hwnd, Some(tab_proc), 1, 0) };
        }
        _ => {}
    }
    BOOL::from(true)
}

/// Follows a change of Windows' app mode or high contrast: everything redrawn in the new colours.
pub fn setting_changed(hwnd: HWND) {
    let dark = wanted();
    if dark == IS_DARK.get() {
        return;
    }
    IS_DARK.set(dark);
    window(hwnd);
    controls(hwnd);
    // SAFETY: repaints our window, its frame and every child.
    let _ = unsafe { RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN) };
}

/// `WM_ERASEBKGND` in dark: the window's background (light keeps the class brush).
pub fn erase(hwnd: HWND, wparam: WPARAM) -> Option<LRESULT> {
    let theme = current();
    if !theme.dark {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: fills our window's client area on the DC the message hands us.
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
        FillRect(HDC(wparam.0 as _), &rect, theme.background_brush);
    }
    Some(LRESULT(1))
}

/// `WM_CTLCOLOREDIT`, `WM_CTLCOLORLISTBOX` and `WM_CTLCOLORBTN` in dark: boxes in the surface colour,
/// buttons' corners in the background (light keeps the defaults).
pub fn box_colours(wparam: WPARAM, button: bool) -> Option<LRESULT> {
    let theme = current();
    if !theme.dark {
        return None;
    }
    let hdc = HDC(wparam.0 as _);
    let (colour, brush) = if button { (theme.background, theme.background_brush) } else { (theme.surface, theme.surface_brush) };
    // SAFETY: colours on the device context the control handed us.
    unsafe {
        windows::Win32::Graphics::Gdi::SetBkColor(hdc, colour);
        SetTextColor(hdc, theme.text);
    }
    Some(LRESULT(brush.0 as isize))
}

fn focus_hidden(hwnd: HWND) -> bool {
    send(hwnd, WM_QUERYUISTATE, 0, 0) as u32 & UISF_HIDEFOCUS != 0
}

fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, format: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: draws our own text on a DC we were handed.
    unsafe { DrawTextW(hdc, &mut wide, rect, format) };
}

/// A checkbox's `NM_CUSTOMDRAW` in dark: its themed box and its label in our text colour.
pub fn custom_draw(lparam: LPARAM) -> Option<LRESULT> {
    if !current().dark {
        return None;
    }
    // SAFETY: WM_NOTIFY's lparam points at the sender's NMHDR for this call.
    let header = unsafe { &*(lparam.0 as *const NMHDR) };
    if header.code != NM_CUSTOMDRAW {
        return None;
    }
    // SAFETY: the sender's style.
    let style = unsafe { GetWindowLongW(header.hwndFrom, GWL_STYLE) };
    let mut class = [0u16; 16];
    // SAFETY: reads the sender's class name into our buffer.
    let length = unsafe { GetClassNameW(header.hwndFrom, &mut class) }.max(0) as usize;
    if String::from_utf16_lossy(&class[..length]) != "Button" || style & BS_TYPEMASK != BS_AUTOCHECKBOX {
        return None;
    }
    // SAFETY: a button's NM_CUSTOMDRAW carries an NMCUSTOMDRAW.
    let draw = unsafe { &*(lparam.0 as *const NMCUSTOMDRAW) };
    if draw.dwDrawStage != CDDS_PREPAINT {
        return None;
    }
    draw_checkbox(draw);
    Some(LRESULT(CDRF_SKIPDEFAULT as isize))
}

fn draw_checkbox(draw: &NMCUSTOMDRAW) {
    let theme = current();
    let (hwnd, hdc, rc) = (draw.hdr.hwndFrom, draw.hdc, draw.rc);
    let checked = send(hwnd, BM_GETCHECK, 0, 0) == 1;
    let state = draw.uItemState;
    let disabled = state.contains(CDIS_DISABLED);
    // Normal, hot, pressed, disabled follow each other in both rows.
    let step = if disabled {
        3
    } else if state.contains(CDIS_SELECTED) {
        2
    } else {
        i32::from(state.contains(CDIS_HOT))
    };
    let part_state = if checked { CBS_CHECKEDNORMAL.0 } else { CBS_UNCHECKEDNORMAL.0 } + step;
    let mut text = [0u16; 64];
    // SAFETY: the box from the dark Button theme at the control's DPI, the label in its font,
    // drawn on the DC the control handed us; the theme closed and the font put back after.
    unsafe {
        let length = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        let label = String::from_utf16_lossy(&text[..length]);
        FillRect(hdc, &rc, theme.background_brush);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let handle = OpenThemeDataForDpi(Some(hwnd), w!("Button"), dpi);
        let size = GetThemePartSize(handle, Some(hdc), BP_CHECKBOX.0, part_state, None, TS_DRAW).unwrap_or_default();
        let top = rc.top + (rc.bottom - rc.top - size.cy) / 2;
        let glyph = RECT { left: rc.left, top, right: rc.left + size.cx, bottom: top + size.cy };
        let _ = DrawThemeBackground(handle, hdc, BP_CHECKBOX.0, part_state, &glyph, None);
        let _ = CloseThemeData(handle);

        let font = HGDIOBJ(send(hwnd, WM_GETFONT, 0, 0) as _);
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, if disabled { theme.grey } else { theme.text });
        let mut rect = RECT { left: glyph.right + size.cx / 3, ..rc };
        draw_text(hdc, &label, &mut rect, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        if state.contains(CDIS_FOCUS) && !focus_hidden(hwnd) {
            let mut extent = rect;
            draw_text(hdc, &label, &mut extent, DT_SINGLELINE | DT_NOPREFIX | DT_CALCRECT);
            let height = extent.bottom - extent.top;
            let top = rc.top + (rc.bottom - rc.top - height) / 2;
            let focus = RECT { left: extent.left - 1, top: top - 1, right: extent.right + 1, bottom: top + height + 1 };
            let _ = DrawFocusRect(hdc, &focus);
        }
        SelectObject(hdc, old);
    }
}

/// The option tabs. In dark the whole strip is painted here: a line along the bottom, the selected
/// tab as an open box on it in the surface colour; light is the control's own drawing.
extern "system" fn tab_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
    if current().dark {
        match msg {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                paint_tabs(hwnd);
                return LRESULT(0);
            }
            _ => {}
        }
    }
    // SAFETY: the tab control's own handling of everything else.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn paint_tabs(hwnd: HWND) {
    let theme = current();
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: paints our tab control between BeginPaint and EndPaint, reading its items into
    // our buffers; the font put back after.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut paint);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        FillRect(hdc, &client, theme.background_brush);
        let count = send(hwnd, TCM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let selected = send(hwnd, TCM_GETCURSEL, 0, 0);
        let mut items = Vec::with_capacity(count);
        for i in 0..count {
            let mut rect = RECT::default();
            send(hwnd, TCM_GETITEMRECT, i, (&raw mut rect) as isize);
            let mut text = [0u16; 64];
            let mut item = TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(text.as_mut_ptr()), cchTextMax: text.len() as i32, ..Default::default() };
            send(hwnd, TCM_GETITEMW, i, (&raw mut item) as isize);
            let length = text.iter().position(|c| *c == 0).unwrap_or(text.len());
            items.push((rect, String::from_utf16_lossy(&text[..length])));
        }
        let bottom = items.iter().map(|(r, _)| r.bottom).max().unwrap_or(client.bottom);
        FillRect(hdc, &RECT { top: bottom - 1, bottom, ..client }, theme.line_brush);

        let old = SelectObject(hdc, HGDIOBJ(send(hwnd, WM_GETFONT, 0, 0) as _));
        SetBkMode(hdc, TRANSPARENT);
        for (i, (rect, label)) in items.iter().enumerate() {
            let is_selected = i as isize == selected;
            let mut text_rect = *rect;
            if is_selected {
                // Open at the bottom, joined to what is under it.
                let tab = RECT { top: rect.top - 2, ..*rect };
                FillRect(hdc, &RECT { right: tab.left + 1, ..tab }, theme.line_brush);
                FillRect(hdc, &RECT { left: tab.right - 1, ..tab }, theme.line_brush);
                FillRect(hdc, &RECT { bottom: tab.top + 1, ..tab }, theme.line_brush);
                FillRect(hdc, &RECT { left: tab.left + 1, top: tab.top + 1, right: tab.right - 1, bottom }, theme.surface_brush);
                text_rect.top -= 2;
            }
            SetTextColor(hdc, if is_selected { theme.text } else { theme.grey });
            draw_text(hdc, label, &mut text_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            if is_selected && GetFocus() == hwnd && !focus_hidden(hwnd) {
                let focus = RECT { left: rect.left + 3, top: rect.top + 1, right: rect.right - 3, bottom: rect.bottom - 2 };
                let _ = DrawFocusRect(hdc, &focus);
            }
        }
        SelectObject(hdc, old);
        let _ = EndPaint(hwnd, &paint);
    }
}
