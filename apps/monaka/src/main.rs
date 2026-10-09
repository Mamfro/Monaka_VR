//! The Monaka VR window: pick a game, set its options, start VR. Plain Win32 controls; the work is
//! `monaka_launch` (a session runs on its own thread and reports back through window messages).
//!
//! Left: the games, adding and removing game packs, and the details log. Right: the game's status,
//! where it comes from and what to set before playing, its options (drawn from its option table;
//! "More options" shows the rest), what is ready (SteamVR, the game), and Start VR. Options are
//! saved as they change and shared with `monaka_play`.
#![windows_subsystem = "windows"]

mod screenshot;
mod theme;

use monaka_launch::games::{self, Game, Kind, Launch, Manifest, Opt, Shown, TABS, Values, number_text};
use monaka_launch::session::{self, Console, Event, State, Stop};
use monaka_launch::settings::Settings;
use monaka_launch::{Paths, packs, system};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, DeleteObject, DrawTextW,
    FW_NORMAL, FW_SEMIBOLD, GetDC, GetSysColorBrush, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject,
    SetBkColor, SetTextColor, COLOR_WINDOW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Controls::{
    EM_REPLACESEL, EM_SETSEL, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR, TCIF_TEXT, TCITEMW,
    TCM_DELETEALLITEMS, TCM_GETCURSEL, TCM_INSERTITEMW, TCM_SETCURSEL, TCN_SELCHANGE, TRACKBAR_CLASSW, WC_COMBOBOXW, WC_TABCONTROLW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

/// A session event is waiting in the receiver.
const WM_SESSION: u32 = WM_APP + 1;
/// Checks SteamVR and the game every so often.
const TIMER_CHECKS: usize = 1;
/// Takes the `--screenshot` picture once the window has drawn.
const TIMER_SCREENSHOT: usize = 2;

const ID_GAMES: i32 = 1;
const ID_START: i32 = 2;
const ID_START_GAME: i32 = 3;
const ID_START_STEAMVR: i32 = 4;
const ID_LOGS: i32 = 5;
const ID_TABS: i32 = 6;
const ID_DETAILS: i32 = 7;
/// Grey text: the status meaning and the readiness lines use these.
const ID_GREY: i32 = 8;
const ID_RESET: i32 = 9;
const ID_ADD_PACK: i32 = 10;
const ID_REMOVE_PACK: i32 = 11;
const ID_LABEL: i32 = 200;
const ID_CONTROL: i32 = 300;
const ID_VALUE: i32 = 400;
/// Option help lines (grey).
const ID_HELP: i32 = 500;

const TBM_GETPOS: u32 = WM_USER;
const TBM_SETPOS: u32 = WM_USER + 5;
const TBM_SETRANGEMIN: u32 = WM_USER + 7;
const TBM_SETRANGEMAX: u32 = WM_USER + 8;
const TBS_NOTICKS: u32 = 0x10;

/// SteamVR's Steam app id.
const STEAMVR_APP: u32 = 250820;

fn main() {
    // SAFETY: process-wide setup before any window exists; failures leave the defaults.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let controls = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES | ICC_TAB_CLASSES };
        let _ = InitCommonControlsEx(&controls);
    }
    let screenshot = screenshot::request();
    theme::start(screenshot.as_ref().and_then(|r| r.dark));
    let Some(hwnd) = create_main_window(screenshot.is_some()) else { return };
    theme::window(hwnd);
    APP.with(|cell| *cell.borrow_mut() = Some(App::new(hwnd)));
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            let mut selected = app.selected;
            if let Some(request) = &screenshot {
                app.tab = request.tab.as_deref().and_then(|name| TABS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name))).map_or(Shown::Main, |(t, _)| *t);
                selected = request.game.as_deref().and_then(|id| app.list.iter().position(|g| g.id == id)).unwrap_or(selected);
                app.screenshot = Some(request.file.clone());
            }
            app.select(selected);
            app.refresh_checks();
        }
    });
    // SAFETY: shows our window and runs the message loop on this thread.
    unsafe {
        if screenshot.is_some() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), TIMER_SCREENSHOT, 1000, None);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        }
        SetTimer(Some(hwnd), TIMER_CHECKS, 1500, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The main window; `off_screen` (for `--screenshot`) puts it where no monitor shows it.
fn create_main_window(off_screen: bool) -> Option<HWND> {
    // SAFETY: registers our class and creates the top-level window with owned strings.
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).ok()?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszClassName: w!("monaka.window"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return None;
        }
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let hwnd = CreateWindowExW(WS_EX_CONTROLPARENT, w!("monaka.window"), w!("Monaka VR"), style, CW_USEDEFAULT, CW_USEDEFAULT, 100, 100, None, None, Some(instance.into()), None).ok()?;
        // Size for the monitor's DPI now that the window is on one.
        let dpi = GetDpiForWindow(hwnd);
        let mut rect = RECT { left: 0, top: 0, right: scale(940, dpi), bottom: scale(700, dpi) };
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut rect, style, false, WS_EX_CONTROLPARENT, dpi);
        let (flags, x, y) = if off_screen { (SWP_NOZORDER | SWP_NOACTIVATE, -20_000, -20_000) } else { (SWP_NOMOVE | SWP_NOZORDER, 0, 0) };
        let _ = SetWindowPos(hwnd, None, x, y, rect.right - rect.left, rect.bottom - rect.top, flags);
        Some(hwnd)
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Colours and drawing come before the app: controls ask while it is handling a message.
        WM_CTLCOLORSTATIC => return static_colours(wparam, lparam),
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            if let Some(result) = theme::box_colours(wparam, msg == WM_CTLCOLORBTN) {
                return result;
            }
        }
        WM_ERASEBKGND => {
            if let Some(result) = theme::erase(hwnd, wparam) {
                return result;
            }
        }
        WM_NOTIFY => {
            if let Some(result) = theme::custom_draw(lparam) {
                return result;
            }
        }
        WM_SETTINGCHANGE => theme::setting_changed(hwnd),
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // Messages sent while the app is already handling one (a control notifying as it is changed)
    // go to the default handling.
    let handled = APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().and_then(|app| app.handle(msg, wparam, lparam))));
    // SAFETY: the default window procedure for our own window.
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

/// The window's background behind every static, checkbox and slider (the read-only log is a box);
/// grey text for help lines.
fn static_colours(wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let hdc = HDC(wparam.0 as _);
    let theme = theme::current();
    // SAFETY: colours on the device context the control handed us; the theme's brushes live on.
    unsafe {
        let id = GetDlgCtrlID(HWND(lparam.0 as _));
        let (colour, brush) = if id == ID_DETAILS { (theme.surface, theme.surface_brush) } else { (theme.background, theme.background_brush) };
        SetBkColor(hdc, colour);
        SetTextColor(hdc, if (ID_HELP..ID_HELP + 100).contains(&id) || id == ID_GREY { theme.grey } else { theme.text });
        LRESULT(brush.0 as isize)
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: a message to one of our own controls; pointer arguments are owned by the caller.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0 }
}

/// An option's help line with these values: why it is off now (another option turns it off), or
/// its own help and what it turns off.
fn help_text(game: &Manifest, values: &Values, opt: &Opt) -> String {
    if let Some((cause, rule)) = values.turned_off(game, opt.id) {
        return format!("Off while {} is {}. {}", cause.label, value_name(cause, rule.value), rule.reason);
    }
    match opt.turns_off.and_then(|rule| game.option(rule.target).map(|target| (rule, target))) {
        Some((rule, target)) => format!("{} {} turns off {}. {}", opt.help, set_to(opt, rule.value), target.label, rule.reason),
        None => opt.help.to_owned(),
    }
}

/// How the window names `value` of `opt`: on or off, or a choice's label.
fn value_name(opt: &Opt, value: &str) -> String {
    match opt.kind {
        Kind::Switch { .. } => (if value == "1" { "on" } else { "off" }).to_owned(),
        Kind::Choice { choices, .. } => choices.iter().find(|(id, _)| *id == value).map_or(value, |(_, name)| name).to_owned(),
        _ => value.to_owned(),
    }
}

/// "Turning it on", "Turning it off" or "Set to <choice>,".
fn set_to(opt: &Opt, value: &str) -> String {
    match opt.kind {
        Kind::Switch { .. } => format!("Turning it {}", value_name(opt, value)),
        _ => format!("Set to {},", value_name(opt, value)),
    }
}

fn set_text(hwnd: HWND, text: &str) {
    // SAFETY: sets a control's text from an owned string.
    let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(text)) };
}

fn text_of(hwnd: HWND) -> String {
    let mut buffer = vec![0u16; 4096];
    // SAFETY: reads a control's text into our buffer.
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
}

fn place(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    // SAFETY: moves one of our own controls.
    let _ = unsafe { MoveWindow(hwnd, x, y, width.max(0), height.max(0), true) };
}

fn show(hwnd: HWND, visible: bool) {
    // SAFETY: shows or hides one of our own controls.
    let _ = unsafe { ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE }) };
}

fn enable(hwnd: HWND, enabled: bool) {
    // SAFETY: enables or disables one of our own controls.
    let _ = unsafe { EnableWindow(hwnd, enabled) };
}

fn destroy(hwnd: HWND) {
    // SAFETY: destroys one of our own child controls.
    let _ = unsafe { DestroyWindow(hwnd) };
}

fn child(parent: HWND, class: PCWSTR, text: &str, style: u32, id: i32) -> HWND {
    // SAFETY: creates a child control of our window with an owned title; placed later by layout.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as isize as _)),
            None,
            None,
        )
        .unwrap_or_default()
    }
}

fn static_text(parent: HWND, text: &str, id: i32) -> HWND {
    // SS_LEFT | SS_NOPREFIX: left-aligned, `&` shown as is.
    child(parent, w!("STATIC"), text, 0x80, id)
}

fn button(parent: HWND, text: &str, id: i32) -> HWND {
    child(parent, w!("BUTTON"), text, WS_TABSTOP.0 | BS_PUSHBUTTON as u32, id)
}

/// Asks an OK/Cancel question (Cancel the default); true for OK.
fn confirm(owner: HWND, title: &str, question: &str, style: MESSAGEBOX_STYLE) -> bool {
    // SAFETY: a modal message box over our window with owned strings.
    unsafe { MessageBoxW(Some(owner), &HSTRING::from(question), &HSTRING::from(title), MB_OKCANCEL | MB_DEFBUTTON2 | style) == IDOK }
}

/// The player's pick of a game pack file (a .zip, or a lone .dll), if any.
fn pick_pack_file(owner: HWND) -> Option<PathBuf> {
    let mut file = vec![0u16; 1024];
    let filter: Vec<u16> = "Game packs (*.zip, *.dll)\0*.zip;*.dll\0\0".encode_utf16().collect();
    let title = HSTRING::from("Add game pack");
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: the dialog writes the chosen path into our buffer, which outlives the call.
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return None;
    }
    let length = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..length])))
}

/// Fills the games list box with `games`.
fn fill_games(list: HWND, games: &[Game]) {
    send(list, LB_RESETCONTENT, 0, 0);
    for game in games {
        let item = HSTRING::from(format!("{}  ({})", game.name, game.status.label()));
        send(list, LB_ADDSTRING, 0, item.as_ptr() as isize);
    }
}

struct Fonts {
    normal: HFONT,
    small: HFONT,
    title: HFONT,
    big: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let font = |points: i32, weight: u32| {
            // SAFETY: creates a font this value owns (deleted in Drop).
            unsafe {
                CreateFontW(
                    -(points * dpi as i32 / 72),
                    0,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    0,
                    w!("Segoe UI"),
                )
            }
        };
        Fonts { normal: font(9, FW_NORMAL.0), small: font(8, FW_NORMAL.0), title: font(16, FW_SEMIBOLD.0), big: font(12, FW_SEMIBOLD.0) }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.normal, self.small, self.title, self.big] {
            // SAFETY: deletes fonts this value created, after the controls moved to new ones.
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        }
    }
}

/// Sets the option tabs to `names`, in order.
fn set_tabs(tabs: HWND, names: &[&str]) {
    send(tabs, TCM_DELETEALLITEMS, 0, 0);
    for (i, name) in names.iter().enumerate() {
        let mut text: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let item = TCITEMW { mask: TCIF_TEXT, pszText: windows::core::PWSTR(text.as_mut_ptr()), ..Default::default() };
        send(tabs, TCM_INSERTITEMW, i, &item as *const TCITEMW as isize);
    }
}

fn set_font(hwnd: HWND, font: HFONT) {
    send(hwnd, WM_SETFONT, font.0 as usize, 1);
}

/// The height `text` needs in `font` when wrapped to `width` pixels.
fn text_height(window: HWND, font: HFONT, text: &str, width: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
    // SAFETY: measures on the window's DC with our font selected, then puts the old one back.
    unsafe {
        let hdc = GetDC(Some(window));
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        DrawTextW(hdc, &mut wide, &mut rect, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(hdc, old);
        ReleaseDC(Some(window), hdc);
    }
    rect.bottom
}

/// One option's controls.
struct Row {
    opt: &'static Opt,
    label: HWND,
    control: HWND,
    /// The number beside a slider.
    value: Option<HWND>,
    help: HWND,
}

/// A session in progress.
struct Running {
    stop: Stop,
    events: Receiver<Event>,
}

struct App {
    hwnd: HWND,
    dpi: u32,
    fonts: Fonts,
    paths: Paths,
    settings: Settings,
    libraries: Vec<PathBuf>,
    /// The games on offer (Monaka's own built ones, then the packs'); may be empty.
    list: &'static [Game],
    selected: usize,
    /// The chosen tab, and the tabs the game has options in ([`TABS`] order).
    tab: Shown,
    tabs_shown: Vec<Shown>,

    games: HWND,
    add_pack: HWND,
    remove_pack: HWND,
    details_label: HWND,
    details: HWND,
    title: HWND,
    status: HWND,
    status_meaning: HWND,
    /// Where the game comes from: Monaka's own, or a pack's word on who made it.
    origin: HWND,
    before: HWND,
    rows: Vec<Row>,
    /// The option tabs ("Main", "More"): only the strip of tabs; the rows sit under it.
    tabs: HWND,
    steamvr_line: HWND,
    start_steamvr: HWND,
    game_line: HWND,
    start_game: HWND,
    start: HWND,
    logs: HWND,
    /// Puts the game's options back to their defaults.
    reset: HWND,
    state_line: HWND,

    running: Option<Running>,
    run_folder: Option<PathBuf>,
    closing: bool,
    steamvr_up: bool,
    /// Another session's viewer is running (not ours).
    other_session: bool,
    game_up: bool,
    game_installed: bool,
    /// The smallest client size the layout fits in (pixels).
    min_client: (i32, i32),
    /// `--screenshot`: where the picture goes.
    screenshot: Option<PathBuf>,
}

impl App {
    fn new(hwnd: HWND) -> App {
        // SAFETY: our window's DPI.
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let settings = Settings::load();
        let paths = Paths::locate();
        let catalogue = games::rescan(&paths);
        let list = catalogue.games;
        let selected = settings.selected.as_deref().and_then(|id| list.iter().position(|g| g.id == id)).unwrap_or(0);
        let games = child(hwnd, w!("LISTBOX"), "", WS_TABSTOP.0 | WS_BORDER.0 | WS_VSCROLL.0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32, ID_GAMES);
        fill_games(games, list);
        let details = child(
            hwnd,
            w!("EDIT"),
            "",
            WS_BORDER.0 | WS_VSCROLL.0 | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
            ID_DETAILS,
        );
        let app = App {
            hwnd,
            dpi,
            fonts: Fonts::new(dpi),
            paths,
            settings,
            libraries: system::steam_libraries(),
            list,
            selected,
            tab: Shown::Main,
            tabs_shown: Vec::new(),
            games,
            add_pack: button(hwnd, "Add game pack...", ID_ADD_PACK),
            remove_pack: button(hwnd, "Remove pack", ID_REMOVE_PACK),
            details_label: static_text(hwnd, "Details", 0),
            details,
            title: static_text(hwnd, "", 0),
            status: static_text(hwnd, "", 0),
            status_meaning: static_text(hwnd, "", ID_GREY),
            origin: static_text(hwnd, "", ID_GREY),
            before: static_text(hwnd, "", 0),
            rows: Vec::new(),
            tabs: child(hwnd, WC_TABCONTROLW, "", WS_TABSTOP.0 | WS_CLIPSIBLINGS.0, ID_TABS),
            steamvr_line: static_text(hwnd, "", 0),
            start_steamvr: button(hwnd, "Start SteamVR", ID_START_STEAMVR),
            game_line: static_text(hwnd, "", 0),
            start_game: button(hwnd, "Start game", ID_START_GAME),
            start: child(hwnd, w!("BUTTON"), "Start VR", WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32, ID_START),
            logs: button(hwnd, "Open logs", ID_LOGS),
            reset: button(hwnd, "Reset to defaults", ID_RESET),
            state_line: static_text(hwnd, "", 0),
            running: None,
            run_folder: None,
            closing: false,
            steamvr_up: false,
            other_session: false,
            game_up: false,
            game_installed: true,
            min_client: (0, 0),
            screenshot: None,
        };
        app.apply_fonts();
        theme::controls(hwnd);
        for problem in &catalogue.problems {
            app.detail(problem);
        }
        app
    }

    /// The selected game; none while no game is on offer.
    fn game(&self) -> Option<&'static Game> {
        self.list.get(self.selected)
    }

    /// Reads the games again after a pack came or went, and shows `select` (a game id), else the
    /// game shown before, else the first.
    fn reload(&mut self, select: Option<&str>) {
        let before = self.game().map(|g| g.id);
        let catalogue = games::rescan(&self.paths);
        self.list = catalogue.games;
        for problem in &catalogue.problems {
            self.detail(problem);
        }
        fill_games(self.games, self.list);
        let index = select.or(before).and_then(|id| self.list.iter().position(|g| g.id == id)).unwrap_or(0);
        self.select(index);
    }

    /// Unpacks a pack the player picks, says what it is with the warning, and adds it on OK.
    fn add_pack(&mut self) {
        let Some(file) = pick_pack_file(self.hwnd) else { return };
        let staged = match packs::stage(&file, &self.paths.packs) {
            Ok(staged) => staged,
            Err(why) => return self.say(&format!("Game pack not added: {why}"), true),
        };
        let clashes = games::clashes(staged.manifests(), staged.replaces().as_deref());
        if !clashes.is_empty() {
            return self.say(&format!("Game pack not added: {}", clashes.join(" ")), true);
        }
        let replaces = if staged.replaces().is_some() { format!("\n\nIt replaces the installed {}.", staged.name()) } else { String::new() };
        let question = format!(
            "Add {} (game pack {})?{replaces}\n\nGame packs run their own code inside the game. Only add packs from people you trust.",
            staged.names(),
            staged.credit()
        );
        if !confirm(self.hwnd, "Add game pack", &question, MB_ICONWARNING) {
            return self.say("Game pack not added.", false);
        }
        let names = staged.names();
        let first = staged.manifests().next().map(|m| m.id.to_owned());
        match staged.install() {
            Ok(folder) => {
                self.say(&format!("Added {names} ({}).", folder.display()), true);
                self.reload(first.as_deref());
            }
            Err(why) => self.say(&format!("Game pack not added: {why}"), true),
        }
    }

    /// Removes the selected game's pack, on OK.
    fn remove_pack(&mut self) {
        let Some(pack) = self.game().filter(|g| g.removable()).map(Game::pack) else { return };
        let folder = pack.folder().to_owned();
        let names = games::names_in(&folder);
        let question = format!("Remove the game pack {names}?\n\nIts folder is deleted: {}\nYour options for its games are kept.", folder.display());
        if !confirm(self.hwnd, "Remove game pack", &question, MB_ICONQUESTION) {
            return;
        }
        match packs::remove(&folder, &self.paths.packs) {
            Ok(()) => {
                self.say(&format!("Removed {names}."), true);
                self.reload(None);
            }
            Err(why) => self.say(&why, true),
        }
    }

    fn apply_fonts(&self) {
        let normal = [
            self.games,
            self.add_pack,
            self.remove_pack,
            self.details_label,
            self.details,
            self.status,
            self.before,
            self.tabs,
            self.steamvr_line,
            self.start_steamvr,
            self.game_line,
            self.start_game,
            self.logs,
            self.reset,
            self.state_line,
        ];
        for hwnd in normal {
            set_font(hwnd, self.fonts.normal);
        }
        set_font(self.status_meaning, self.fonts.small);
        set_font(self.origin, self.fonts.small);
        set_font(self.title, self.fonts.title);
        set_font(self.start, self.fonts.big);
        for row in &self.rows {
            for hwnd in [Some(row.label), Some(row.control), row.value].into_iter().flatten() {
                set_font(hwnd, self.fonts.normal);
            }
            set_font(row.help, self.fonts.small);
        }
    }

    /// Shows game `index`: its texts and a fresh set of option controls.
    fn select(&mut self, index: usize) {
        self.selected = index.min(self.list.len().saturating_sub(1));
        for row in self.rows.drain(..) {
            for hwnd in [Some(row.label), Some(row.control), row.value, Some(row.help)].into_iter().flatten() {
                destroy(hwnd);
            }
        }
        enable(self.remove_pack, self.running.is_none() && self.game().is_some_and(Game::removable));
        let Some(game) = self.game() else {
            // No game on offer yet: a Monaka without its own builds and no pack added.
            set_text(self.title, "No games yet");
            set_text(self.status, "");
            set_text(self.status_meaning, "Add a game pack (a .zip from the people who made it) with Add game pack.");
            for hwnd in [self.origin, self.before] {
                set_text(hwnd, "");
            }
            self.tabs_shown.clear();
            show(self.tabs, false);
            self.refresh_checks();
            self.layout();
            return;
        };
        send(self.games, LB_SETCURSEL, self.selected, 0);
        set_text(self.title, game.name);
        set_text(self.status, &format!("Status: {}", game.status.label()));
        set_text(self.status_meaning, game.status.meaning());
        set_text(self.origin, &game.credit());
        set_text(self.before, &format!("Before you play: {}", game.before));
        let values = self.settings.values(game.id);
        // Command-line-only options get no row; a row's control id is its index in `rows`.
        for opt in game.options.iter().filter(|o| o.shown != Shown::Line) {
            let i = self.rows.len() as i32;
            let label = static_text(self.hwnd, opt.label, ID_LABEL + i);
            let current = values.text(game, opt.id);
            let (control, value) = match opt.kind {
                Kind::Choice { choices, window, .. } => {
                    let combo = child(self.hwnd, WC_COMBOBOXW, "", WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32, ID_CONTROL + i);
                    for (_, choice) in &choices[..window] {
                        let text = HSTRING::from(*choice);
                        send(combo, CB_ADDSTRING, 0, text.as_ptr() as isize);
                    }
                    send(combo, CB_SETCURSEL, choices[..window].iter().position(|(id, _)| *id == current).unwrap_or(0), 0);
                    (combo, None)
                }
                Kind::Switch { .. } => {
                    let check = child(self.hwnd, w!("BUTTON"), "On", WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32, ID_CONTROL + i);
                    send(check, BM_SETCHECK, usize::from(current == "1"), 0);
                    (check, None)
                }
                Kind::Number { min, max, step, .. } => {
                    let slider = child(self.hwnd, TRACKBAR_CLASSW, "", WS_TABSTOP.0 | TBS_NOTICKS, ID_CONTROL + i);
                    send(slider, TBM_SETRANGEMIN, 0, 0);
                    send(slider, TBM_SETRANGEMAX, 0, ((max - min) / step).round() as isize);
                    let position = ((current.parse::<f64>().unwrap_or(min) - min) / step).round() as isize;
                    send(slider, TBM_SETPOS, 1, position);
                    (slider, Some(static_text(self.hwnd, &current, ID_VALUE + i)))
                }
                Kind::Text | Kind::Size => (child(self.hwnd, w!("EDIT"), &current, WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32, ID_CONTROL + i), None),
            };
            let help = static_text(self.hwnd, opt.help, ID_HELP + i);
            self.rows.push(Row { opt, label, control, value, help });
        }
        // The tabs this game has options in; a tab it lacks falls back to Main.
        self.tabs_shown = TABS.iter().map(|(t, _)| *t).filter(|t| game.options.iter().any(|o| o.shown == *t)).collect();
        if !self.tabs_shown.contains(&self.tab) {
            self.tab = Shown::Main;
        }
        set_tabs(self.tabs, &TABS.iter().filter(|(t, _)| self.tabs_shown.contains(t)).map(|(_, n)| *n).collect::<Vec<_>>());
        show(self.tabs, self.tabs_shown.len() > 1);
        send(self.tabs, TCM_SETCURSEL, self.tabs_shown.iter().position(|t| *t == self.tab).unwrap_or(0), 0);
        self.apply_fonts();
        theme::controls(self.hwnd);
        self.settings.selected = Some(game.id.to_owned());
        let _ = self.settings.save();
        self.refresh_checks();
        self.layout();
    }

    /// Reads option `i`'s control into the settings and saves them.
    fn option_changed(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else { return };
        let Some(game) = self.game() else { return };
        let value = match row.opt.kind {
            Kind::Choice { choices, .. } => {
                let index = send(row.control, CB_GETCURSEL, 0, 0);
                choices.get(index.max(0) as usize).map(|(id, _)| (*id).to_owned()).unwrap_or_default()
            }
            Kind::Switch { .. } => (if send(row.control, BM_GETCHECK, 0, 0) == 1 { "1" } else { "0" }).to_owned(),
            Kind::Number { min, step, .. } => {
                let text = number_text(min + send(row.control, TBM_GETPOS, 0, 0) as f64 * step);
                if let Some(label) = row.value {
                    set_text(label, &text);
                }
                text
            }
            Kind::Text | Kind::Size => text_of(row.control),
        };
        let choice = matches!(row.opt.kind, Kind::Choice { .. } | Kind::Switch { .. });
        self.settings.values.entry(game.id.to_owned()).or_default().set(row.opt.id, value);
        let _ = self.settings.save();
        if choice {
            // Another option may come or go with this one ([`games::Opt::when`]).
            self.layout();
        }
    }

    /// Places every control for the current size and DPI.
    fn layout(&mut self) {
        let mut client = RECT::default();
        // SAFETY: our window's client area.
        let _ = unsafe { GetClientRect(self.hwnd, &mut client) };
        let s = |v: i32| scale(v, self.dpi);
        let (width, height) = (client.right, client.bottom);
        let margin = s(14);
        let left_width = s(250);

        // Left: the games, adding and removing packs, then the details.
        let list_height = s(26) * self.list.len().max(1) as i32 + s(6);
        place(self.games, margin, margin, left_width, list_height);
        let packs_top = margin + list_height + s(8);
        place(self.add_pack, margin, packs_top, s(140), s(28));
        place(self.remove_pack, margin + s(148), packs_top, left_width - s(148), s(28));
        let details_top = packs_top + s(28) + s(14);
        place(self.details_label, margin, details_top, left_width, s(20));
        place(self.details, margin, details_top + s(22), left_width, height - details_top - s(22) - margin);

        // Right, from the top.
        let x = margin + left_width + s(20);
        let w = (width - x - margin).max(s(300));
        let mut y = margin - s(4);
        place(self.title, x, y, w, s(34));
        y += s(38);
        place(self.status, x, y, w, s(20));
        y += s(20);
        let meaning = text_height(self.hwnd, self.fonts.small, &text_of(self.status_meaning), w);
        place(self.status_meaning, x, y, w, meaning);
        y += meaning + s(6);
        let origin = text_of(self.origin);
        show(self.origin, !origin.is_empty());
        if !origin.is_empty() {
            let h = text_height(self.hwnd, self.fonts.small, &origin, w);
            place(self.origin, x, y, w, h);
            y += h + s(6);
        }
        let before = text_height(self.hwnd, self.fonts.normal, &text_of(self.before), w);
        place(self.before, x, y, w, before);
        y += before + s(16);

        let label_width = s(180);
        let control_x = x + label_width;
        // Wide windows leave the controls at a readable width.
        let control_width = (w - label_width).min(s(460));
        let value_width = s(48);
        if self.tabs_shown.len() > 1 {
            place(self.tabs, x, y, w, s(30));
            y += s(40);
        }
        let tab = self.tab;
        // Rows exist only for a selected game.
        let game = self.game();
        let values = game.map(|g| self.settings.values(g.id)).unwrap_or_default();
        for row in game.map_or(&[][..], |_| &self.rows[..]) {
            let Some(game) = game else { break };
            // An option that means nothing with the others' values (the aiming hand without a
            // pointing controller) is left out.
            let visible = row.opt.shown == tab && values.applies(game, row.opt);
            for hwnd in [Some(row.label), Some(row.control), row.value, Some(row.help)].into_iter().flatten() {
                show(hwnd, visible);
            }
            if !visible {
                continue;
            }
            // The help line says what this option turns off, or why it is off now; a switch another
            // option turns off is greyed out, shown off.
            let help_text = help_text(game, &values, row.opt);
            if text_of(row.help) != help_text {
                set_text(row.help, &help_text);
            }
            if let Kind::Switch { .. } = row.opt.kind {
                enable(row.control, values.turned_off(game, row.opt.id).is_none());
                send(row.control, BM_SETCHECK, usize::from(values.switch(game, row.opt.id)), 0);
            }
            // Long labels wrap onto a second line.
            let label_height = text_height(self.hwnd, self.fonts.normal, row.opt.label, label_width - s(12)).max(s(20));
            place(row.label, x, y + s(4), label_width - s(12), label_height);
            match row.opt.kind {
                // A drop-down list's height is its open list's.
                Kind::Choice { .. } => place(row.control, control_x, y, control_width, s(240)),
                Kind::Number { .. } => {
                    place(row.control, control_x - s(4), y, control_width - value_width, s(28));
                    if let Some(value) = row.value {
                        place(value, control_x + control_width - value_width + s(4), y + s(4), value_width - s(4), s(20));
                    }
                }
                Kind::Text | Kind::Size => place(row.control, control_x, y, control_width, s(24)),
                Kind::Switch { .. } => place(row.control, control_x, y + s(2), control_width, s(22)),
            }
            let help = text_height(self.hwnd, self.fonts.small, &help_text, control_width);
            place(row.help, control_x, y + s(28), control_width, help);
            y += (s(28) + help).max(label_height + s(4)) + s(10);
        }

        // Right, from the bottom: state, Start VR, readiness.
        let state_height = s(44);
        let start_height = s(44);
        let ready_height = s(28) * 2;
        let bottom_block = ready_height + s(12) + start_height + s(8) + state_height;
        let mut b = (height - margin - bottom_block).max(y + s(8));
        let button_width = s(140);
        place(self.steamvr_line, x, b + s(4), w - button_width - s(10), s(20));
        place(self.start_steamvr, x + w - button_width, b, button_width, s(26));
        b += s(28);
        place(self.game_line, x, b + s(4), w - button_width - s(10), s(20));
        place(self.start_game, x + w - button_width, b, button_width, s(26));
        b += s(28) + s(12);
        place(self.start, x, b, s(220), start_height);
        place(self.logs, x + s(232), b + s(8), s(120), s(28));
        place(self.reset, x + s(362), b + s(8), s(150), s(28));
        b += start_height + s(8);
        place(self.state_line, x, b, w, state_height);

        self.min_client = (s(760), y + s(8) + bottom_block + margin);
        if height < self.min_client.1 && height > 0 {
            self.grow_to_fit();
        }
    }

    /// Makes the window tall enough for the current layout (more options shown).
    fn grow_to_fit(&self) {
        let mut window = RECT::default();
        let mut client = RECT::default();
        // SAFETY: our window's rectangles, then a resize; the WM_SIZE it sends is handled after.
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut window);
            let _ = GetClientRect(self.hwnd, &mut client);
            let extra = self.min_client.1 - client.bottom;
            let _ = SetWindowPos(self.hwnd, None, 0, 0, window.right - window.left, window.bottom - window.top + extra, SWP_NOMOVE | SWP_NOZORDER);
            let _ = PostMessageW(Some(self.hwnd), WM_SIZE, WPARAM(0), LPARAM(0));
        }
    }

    /// Updates what is ready (SteamVR, the game) and what may be pressed.
    fn refresh_checks(&mut self) {
        self.steamvr_up = system::steamvr_running();
        let idle = self.running.is_none();
        self.other_session = idle && system::viewer_running();
        set_text(
            self.steamvr_line,
            match (self.steamvr_up, self.other_session) {
                (_, true) => "Another VR session is already running. Stop it first.",
                (true, false) => "SteamVR: running",
                (false, false) => "SteamVR: not running. Start it and turn on your headset.",
            },
        );
        enable(self.start_steamvr, !self.steamvr_up);
        let Some(game) = self.game() else {
            set_text(self.game_line, "Game: none yet");
            enable(self.start_game, false);
            enable(self.start, !idle && !self.closing);
            return;
        };
        self.game_up = !system::game_processes(game.executables, game.steam_app).is_empty();
        self.game_installed = self.libraries.is_empty() || system::steam_installed(&self.libraries, game.steam_app);
        let game_text = match (self.game_up, self.game_installed, game.launch) {
            (true, _, _) => "Game: running".to_owned(),
            (false, false, _) => "Game: not installed in Steam".to_owned(),
            (false, true, Launch::Auto { .. }) => "Game: not running (Start VR starts it)".to_owned(),
            (false, true, Launch::Manual) => "Game: not running. Start it and load into gameplay first.".to_owned(),
        };
        set_text(self.game_line, &game_text);
        enable(self.start_game, idle && !self.game_up && self.game_installed);
        let game_ready = self.game_up || (self.game_installed && matches!(game.launch, Launch::Auto { .. }));
        enable(self.start, if idle { self.steamvr_up && game_ready && !self.other_session } else { !self.closing });
    }

    fn set_running_controls(&self, running: bool) {
        enable(self.games, !running);
        for row in &self.rows {
            enable(row.control, !running);
        }
        enable(self.reset, !running);
        enable(self.add_pack, !running);
        enable(self.remove_pack, !running && self.game().is_some_and(Game::removable));
        set_text(self.start, if running { "Stop VR" } else { "Start VR" });
    }

    fn start_session(&mut self) {
        let stop = match Stop::new() {
            Ok(stop) => stop,
            Err(why) => return self.say(&why, true),
        };
        let Some(game) = self.game() else { return };
        let (send_event, events) = mpsc::channel();
        let values = self.settings.values(game.id);
        let paths = self.paths.clone();
        let session_stop = stop.clone();
        let hwnd = self.hwnd.0 as isize;
        set_text(self.details, "");
        self.run_folder = None;
        std::thread::spawn(move || {
            session::run(game, &values, &paths, Console::Hidden, &session_stop, &mut |event| {
                let _ = send_event.send(event);
                // SAFETY: posting to our window from this thread; the window outlives the session
                // (closing waits for its end).
                let _ = unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_SESSION, WPARAM(0), LPARAM(0)) };
            });
        });
        self.running = Some(Running { stop, events });
        self.set_running_controls(true);
        self.say("Starting...", false);
        self.refresh_checks();
    }

    fn stop_session(&mut self) {
        if let Some(running) = &self.running {
            running.stop.request();
            set_text(self.start, "Stopping...");
            enable(self.start, false);
        }
    }

    /// The state line, and the details.
    fn say(&self, line: &str, detail: bool) {
        set_text(self.state_line, line);
        if detail {
            self.detail(line);
        }
    }

    fn detail(&self, line: &str) {
        // Keep the log from growing without end.
        if send(self.details, WM_GETTEXTLENGTH, 0, 0) > 200_000 {
            set_text(self.details, "");
        }
        let text = HSTRING::from(format!("{line}\r\n"));
        send(self.details, EM_SETSEL, usize::MAX >> 1, isize::MAX >> 1);
        send(self.details, EM_REPLACESEL, 0, text.as_ptr() as isize);
    }

    fn session_events(&mut self) {
        let Some(running) = &self.running else { return };
        let events: Vec<Event> = running.events.try_iter().collect();
        for event in events {
            match event {
                Event::State(state) => self.session_state(state),
                Event::Info(line) => self.say(&line, true),
                Event::Warning(line) => self.say(&line, true),
                Event::Failed(line) => {
                    self.say(&line, false);
                    self.detail(&format!("Error: {line}"));
                }
                Event::Detail(line) => self.detail(&format!("  {line}")),
                Event::RunFolder(folder) => self.run_folder = Some(folder),
            }
        }
    }

    fn session_state(&mut self, state: State) {
        match state {
            State::WaitingForGame => self.say("Waiting for the game...", false),
            State::Attaching => self.say("Attaching to the game...", false),
            State::Running => self.say("VR is running. Put on your headset. Recentre with the headset's own control.", false),
            State::Stopping => self.say("Stopping VR...", false),
            State::Stopped => {
                let failed = text_of(self.state_line);
                let ended_well = matches!(failed.as_str(), "VR is running. Put on your headset. Recentre with the headset's own control." | "Stopping VR..." | "Starting...");
                if ended_well {
                    self.say("VR stopped.", true);
                }
                self.running = None;
                self.set_running_controls(false);
                if self.closing {
                    // SAFETY: closes our window now that the producer is stopped.
                    let _ = unsafe { DestroyWindow(self.hwnd) };
                    return;
                }
                self.refresh_checks();
            }
        }
    }

    /// The game's options back to their defaults (forgotten in the settings; the rows rebuilt).
    fn reset_defaults(&mut self) {
        let Some(game) = self.game() else { return };
        self.settings.values.remove(game.id);
        let _ = self.settings.save();
        self.select(self.selected);
        self.say(&format!("{}: options back to their defaults.", game.name), false);
    }

    fn open_logs(&self) {
        let folder = self.run_folder.clone().unwrap_or_else(|| self.paths.runs.clone());
        let _ = std::fs::create_dir_all(&folder);
        if let Err(why) = system::open(&folder.to_string_lossy()) {
            self.say(&why, true);
        }
    }

    fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as i32;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    ID_GAMES if code == LBN_SELCHANGE => {
                        let index = send(self.games, LB_GETCURSEL, 0, 0);
                        if index >= 0 && index as usize != self.selected {
                            self.select(index as usize);
                        }
                    }
                    ID_START if code == BN_CLICKED => {
                        if self.running.is_some() {
                            self.stop_session();
                        } else {
                            self.start_session();
                        }
                    }
                    ID_START_GAME if code == BN_CLICKED => {
                        if let Some(game) = self.game() {
                            match system::launch_steam_game(game.steam_app) {
                                Ok(()) => self.say(&format!("Starting {} through Steam...", game.name), false),
                                Err(why) => self.say(&why, true),
                            }
                        }
                    }
                    ID_ADD_PACK if code == BN_CLICKED => self.add_pack(),
                    ID_REMOVE_PACK if code == BN_CLICKED => self.remove_pack(),
                    ID_START_STEAMVR if code == BN_CLICKED => {
                        if let Err(why) = system::launch_steam_game(STEAMVR_APP) {
                            self.say(&why, true);
                        }
                    }
                    ID_LOGS if code == BN_CLICKED => self.open_logs(),
                    ID_RESET if code == BN_CLICKED => self.reset_defaults(),
                    id if (ID_CONTROL..ID_CONTROL + 100).contains(&id) => {
                        let changed = matches!(code, CBN_SELCHANGE | BN_CLICKED) || code == EN_CHANGE;
                        if changed {
                            self.option_changed((id - ID_CONTROL) as usize);
                        }
                    }
                    _ => return None,
                }
                Some(LRESULT(0))
            }
            WM_NOTIFY => {
                // SAFETY: WM_NOTIFY's lparam points at the sender's NMHDR for this call.
                let header = unsafe { &*(lparam.0 as *const NMHDR) };
                if header.idFrom != ID_TABS as usize || header.code != TCN_SELCHANGE {
                    return None;
                }
                self.tab = self.tabs_shown.get(send(self.tabs, TCM_GETCURSEL, 0, 0).max(0) as usize).copied().unwrap_or(Shown::Main);
                self.layout();
                Some(LRESULT(0))
            }
            WM_HSCROLL => {
                let slider = HWND(lparam.0 as _);
                let index = self.rows.iter().position(|r| r.control == slider)?;
                self.option_changed(index);
                Some(LRESULT(0))
            }
            WM_SESSION => {
                self.session_events();
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == TIMER_SCREENSHOT => {
                if let Some(file) = self.screenshot.take() {
                    let _ = screenshot::save(self.hwnd, &file);
                }
                // SAFETY: the picture is taken; close our window.
                let _ = unsafe { DestroyWindow(self.hwnd) };
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == TIMER_CHECKS => {
                self.refresh_checks();
                Some(LRESULT(0))
            }
            WM_SIZE => {
                self.layout();
                Some(LRESULT(0))
            }
            WM_GETMINMAXINFO => {
                // SAFETY: the system passes a MINMAXINFO for us to fill.
                let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                let mut rect = RECT { left: 0, top: 0, right: self.min_client.0, bottom: self.min_client.1 };
                // SAFETY: converts a client size to a window size for our style and DPI.
                let _ = unsafe { windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut rect, WS_OVERLAPPEDWINDOW, false, WS_EX_CONTROLPARENT, self.dpi) };
                info.ptMinTrackSize.x = rect.right - rect.left;
                info.ptMinTrackSize.y = rect.bottom - rect.top;
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.dpi = ((wparam.0 & 0xffff) as u32).max(96);
                self.fonts = Fonts::new(self.dpi);
                self.apply_fonts();
                // SAFETY: the system passes the suggested window rectangle for the new DPI.
                let r = unsafe { &*(lparam.0 as *const RECT) };
                // SAFETY: moves our window to the suggested rectangle.
                let _ = unsafe { SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE) };
                self.layout();
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                if self.running.is_some() {
                    // Stop the producer before the window goes; the Stopped event closes it.
                    self.closing = true;
                    self.say("Stopping VR before closing...", false);
                    self.stop_session();
                    return Some(LRESULT(0));
                }
                None
            }
            _ => None,
        }
    }
}
