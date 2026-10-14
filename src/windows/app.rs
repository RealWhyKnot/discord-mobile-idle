use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DialogBoxIndirectParamW, DispatchMessageW, EndDialog, GetCursorPos, GetDlgItem, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, LookupIconIdFromDirectoryEx, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetDlgItemTextW, SetForegroundWindow, SetMenuDefaultItem, SetWindowPos,
    TrackPopupMenuEx, TranslateMessage, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DS_SETFONT, DS_SETFOREGROUND,
    ES_AUTOHSCROLL, ES_PASSWORD, HICON, HWND_TOPMOST, IDCANCEL, IDOK, LR_DEFAULTSIZE, MB_ICONINFORMATION, MB_OK,
    MB_SETFOREGROUND, MB_TOPMOST, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SWP_NOMOVE, SWP_NOSIZE,
    SW_SHOWNORMAL, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTALIGN, WM_APP, WM_CLOSE, WM_COMMAND, WM_ENDSESSION,
    WM_INITDIALOG, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_TOPMOST,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use super::service::{OnState, Service};
use super::store::{self, APP};
use super::update::{self, Updater, VERSION};
use super::wide;
use crate::config::{load_config, py_list, Config};
use crate::{error, info};

const START_DELAY: u64 = 30;
const INTERVAL: u64 = 86_400;
const WM_TRAY: u32 = WM_APP + 1;
const WM_STATE: u32 = WM_APP + 2;
const WM_NOTICE: u32 = WM_APP + 3;

const STATES: [&str; 4] = ["connecting", "active", "holding", "error"];
const ICONS: [&[u8]; 4] = [
    include_bytes!("../../packaging/tray-connecting.ico"),
    include_bytes!("../../packaging/tray-active.ico"),
    include_bytes!("../../packaging/tray-holding.ico"),
    include_bytes!("../../packaging/tray-error.ico"),
];

const ID_DETAIL: u32 = 1;
const ID_LOG: u32 = 2;
const ID_SETTINGS: u32 = 3;
const ID_TOKEN: u32 = 4;
const ID_AUTOSTART: u32 = 5;
const ID_RECONNECT: u32 = 6;
const ID_UPDATE: u32 = 7;
const ID_QUIT: u32 = 8;
const ID_EDIT: i32 = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Update {
    Idle,
    Checking,
    Current,
    Failed,
    Ready,
}

struct Shared {
    state: usize,
    detail: String,
    update: Update,
    tag: String,
    staged: Option<PathBuf>,
    notices: Vec<String>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    state: 0,
    detail: String::new(),
    update: Update::Idle,
    tag: String::new(),
    staged: None,
    notices: Vec::new(),
});
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static WAKE: (Mutex<(bool, bool)>, Condvar) = (Mutex::new((false, false)), Condvar::new());

thread_local! {
    static ICON_HANDLES: RefCell<Vec<HICON>> = const { RefCell::new(Vec::new()) };
    static SERVICE: RefCell<Option<Service>> = const { RefCell::new(None) };
    static DIALOG_OPEN: Cell<bool> = const { Cell::new(false) };
    static TOKEN_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn shared() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

fn window() -> HWND {
    WINDOW.load(Ordering::SeqCst) as HWND
}

fn post(message: u32) {
    let hwnd = window();
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, message, 0, 0) };
    }
}

fn copy_into(target: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
    target[..units.len()].copy_from_slice(&units);
    target[units.len()] = 0;
}

fn group_directory(ico: &[u8]) -> Option<Vec<u8>> {
    let count = u16::from_le_bytes(ico.get(4..6)?.try_into().ok()?) as usize;
    let mut out = ico.get(..6)?.to_vec();
    for i in 0..count {
        let at = 6 + 16 * i;
        out.extend_from_slice(ico.get(at..at + 12)?);
        out.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }
    Some(out)
}

pub fn load_icon(ico: &[u8]) -> Option<HICON> {
    let directory = group_directory(ico)?;
    let id = unsafe { LookupIconIdFromDirectoryEx(directory.as_ptr(), 1, 0, 0, LR_DEFAULTSIZE) };
    if id <= 0 {
        return None;
    }
    let at = 6 + 16 * (id as usize - 1);
    let size = u32::from_le_bytes(ico.get(at + 8..at + 12)?.try_into().ok()?) as usize;
    let offset = u32::from_le_bytes(ico.get(at + 12..at + 16)?.try_into().ok()?) as usize;
    let image = ico.get(offset..offset + size)?;
    let icon = unsafe { CreateIconFromResourceEx(image.as_ptr(), size as u32, 1, 0x0003_0000, 0, 0, LR_DEFAULTSIZE) };
    (!icon.is_null()).then_some(icon)
}

fn tray_data() -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window(),
        uID: 1,
        ..Default::default()
    }
}

fn tooltip(detail: &str) -> String {
    format!("{} {} - {}", APP, VERSION, detail)
}

fn add_icon(tip: &str) {
    let state = shared().state;
    let mut data = tray_data();
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = ICON_HANDLES.with(|icons| icons.borrow().get(state).copied().unwrap_or(std::ptr::null_mut()));
    copy_into(&mut data.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
}

fn refresh_icon() {
    let (state, detail) = {
        let shared = shared();
        (shared.state, shared.detail.clone())
    };
    let mut data = tray_data();
    data.uFlags = NIF_ICON | NIF_TIP;
    data.hIcon = ICON_HANDLES.with(|icons| icons.borrow().get(state).copied().unwrap_or(std::ptr::null_mut()));
    copy_into(&mut data.szTip, &tooltip(&detail));
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
}

fn show_notices() {
    let notices = std::mem::take(&mut shared().notices);
    for text in notices {
        let mut data = tray_data();
        data.uFlags = NIF_INFO;
        copy_into(&mut data.szInfo, &text);
        copy_into(&mut data.szInfoTitle, APP);
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } == 0 {
            error!("could not show a notification");
        }
    }
}

fn remove_icon() {
    let data = tray_data();
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

fn notify(text: String) {
    shared().notices.push(text);
    post(WM_NOTICE);
}

fn set_update(state: Update) {
    shared().update = state;
}

fn update_label() -> String {
    let shared = shared();
    match shared.update {
        Update::Idle => "Check for updates".into(),
        Update::Checking => "Checking for updates".into(),
        Update::Current => "No update available".into(),
        Update::Failed => "Update check failed, see the log".into(),
        Update::Ready => format!("Install {} and restart", shared.tag),
    }
}

fn menu_text(text: &str) -> Vec<u16> {
    wide(&text.replace('&', "&&"))
}

fn show_menu(hwnd: HWND) {
    let (detail, checking) = {
        let shared = shared();
        (shared.detail.clone(), shared.update == Update::Checking)
    };
    let checked = if store::autostart_enabled() { MF_CHECKED } else { 0 };
    let update_flags = MF_STRING | if checking { MF_GRAYED } else { 0 };
    let items: Vec<(u32, u32, Option<String>)> = {
        let mut items = vec![
            (MF_STRING | MF_GRAYED, ID_DETAIL, Some(detail)),
            (MF_SEPARATOR, 0, None),
            (MF_STRING, ID_LOG, Some("Open log".into())),
            (MF_STRING, ID_SETTINGS, Some("Edit settings".into())),
            (MF_STRING, ID_TOKEN, Some("Set token".into())),
            (MF_STRING | checked, ID_AUTOSTART, Some("Start with Windows".into())),
            (MF_STRING, ID_RECONNECT, Some("Reconnect".into())),
        ];
        if update::checkable() {
            items.push((update_flags, ID_UPDATE, Some(update_label())));
        }
        items.push((MF_SEPARATOR, 0, None));
        items.push((MF_STRING, ID_QUIT, Some("Quit".into())));
        items
    };
    let command = unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (flags, id, text) in &items {
            let label = text.as_deref().map(menu_text);
            let pointer = label.as_ref().map_or(std::ptr::null(), |l| l.as_ptr());
            AppendMenuW(menu, *flags, *id as usize, pointer);
        }
        SetMenuDefaultItem(menu, ID_LOG, 0);
        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
            point.x,
            point.y,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        command as u32
    };
    run_command(hwnd, command);
}

fn run_command(hwnd: HWND, command: u32) {
    match command {
        ID_LOG => open(&store::path_for("idlebot.log")),
        ID_SETTINGS => open(&store::settings_file()),
        ID_TOKEN => set_token(),
        ID_AUTOSTART => {
            if let Err(e) = store::set_autostart(!store::autostart_enabled()) {
                error!("could not change the autostart setting: {}", e);
            }
        }
        ID_RECONNECT => SERVICE.with(|service| {
            if let Some(service) = service.borrow().as_ref() {
                service.restart();
            }
        }),
        ID_UPDATE => update_click(hwnd),
        ID_QUIT => quit(hwnd),
        _ => {}
    }
}

fn open(path: &Path) {
    let target = wide(&path.to_string_lossy());
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        error!("could not open {}: shell error {}", path.display(), result as isize);
    }
}

fn set_token() {
    if DIALOG_OPEN.get() {
        return;
    }
    let token = ask_token(&store::read_token(), window());
    if token.is_empty() {
        return;
    }
    if let Err(e) = store::write_token(&token) {
        error!("could not save the token: {}", e);
        return;
    }
    SERVICE.with(|service| {
        if let Some(service) = service.borrow().as_ref() {
            service.set_token(token);
        }
    });
}

fn update_click(hwnd: HWND) {
    let (state, staged) = {
        let shared = shared();
        (shared.update, shared.staged.clone())
    };
    if state != Update::Ready {
        let (lock, wake) = &WAKE;
        let mut flags = lock.lock().unwrap_or_else(|e| e.into_inner());
        *flags = (true, true);
        wake.notify_all();
        return;
    }
    let Some(staged) = staged else {
        return;
    };
    let target = std::env::current_exe().unwrap_or_default();
    match update::launch(&staged, &target, std::process::id()) {
        Ok(()) => quit(hwnd),
        Err(e) => error!("could not start the update helper: {}", e),
    }
}

fn quit(hwnd: HWND) {
    if let Some(service) = SERVICE.with(|service| service.borrow_mut().take()) {
        service.stop();
    }
    remove_icon();
    unsafe {
        DestroyWindow(hwnd);
        PostQuitMessage(0);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP => open(&store::path_for("idlebot.log")),
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_STATE => {
            refresh_icon();
            0
        }
        WM_NOTICE => {
            show_notices();
            0
        }
        WM_CLOSE => {
            quit(hwnd);
            0
        }
        WM_ENDSESSION if wparam != 0 => {
            quit(hwnd);
            0
        }
        _ if message != 0 && message == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            let detail = shared().detail.clone();
            add_icon(&tooltip(&detail));
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn create_window() -> Option<HWND> {
    let class = wide("discord-mobile-idle-tray");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = wide(APP);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        (!hwnd.is_null()).then_some(hwnd)
    }
}

fn push_text(out: &mut Vec<u16>, text: &str) {
    out.extend(text.encode_utf16());
    out.push(0);
}

fn push_u32(out: &mut Vec<u16>, value: u32) {
    out.push(value as u16);
    out.push((value >> 16) as u16);
}

fn align(out: &mut Vec<u16>) {
    if out.len() % 2 == 1 {
        out.push(0);
    }
}

fn push_item(out: &mut Vec<u16>, style: u32, rect: [i16; 4], id: u16, class: u16, text: &str) {
    align(out);
    push_u32(out, style | WS_CHILD | WS_VISIBLE);
    push_u32(out, 0);
    out.extend(rect.iter().map(|v| *v as u16));
    out.push(id);
    out.extend([0xFFFF, class]);
    push_text(out, text);
    out.push(0);
}

pub fn token_template() -> Vec<u32> {
    let mut out = Vec::new();
    let style = DS_SETFONT as u32
        | DS_MODALFRAME as u32
        | DS_CENTER as u32
        | DS_SETFOREGROUND as u32
        | WS_POPUP
        | WS_CAPTION
        | WS_SYSMENU;
    push_u32(&mut out, style);
    push_u32(&mut out, WS_EX_TOPMOST);
    out.push(4);
    out.extend([0u16, 0, 200, 62]);
    out.extend([0, 0]);
    push_text(&mut out, APP);
    out.push(9);
    push_text(&mut out, "Segoe UI");
    push_item(
        &mut out,
        0,
        [7, 7, 186, 9],
        u16::MAX,
        0x0082,
        "Paste your Discord token",
    );
    push_item(
        &mut out,
        ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_BORDER | WS_TABSTOP,
        [7, 19, 186, 13],
        ID_EDIT as u16,
        0x0081,
        "",
    );
    push_item(&mut out, 1 | WS_TABSTOP, [89, 41, 50, 14], IDOK as u16, 0x0080, "OK");
    push_item(
        &mut out,
        WS_TABSTOP,
        [143, 41, 50, 14],
        IDCANCEL as u16,
        0x0080,
        "Cancel",
    );
    align(&mut out);
    out.chunks(2)
        .map(|pair| pair[0] as u32 | (pair[1] as u32) << 16)
        .collect()
}

unsafe extern "system" fn token_proc(dialog: HWND, message: u32, wparam: WPARAM, _lparam: LPARAM) -> isize {
    match message {
        WM_INITDIALOG => {
            let initial = TOKEN_TEXT.with(|text| wide(&text.borrow()));
            unsafe {
                SetDlgItemTextW(dialog, ID_EDIT, initial.as_ptr());
                SetWindowPos(dialog, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            }
            1
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            if id == IDOK {
                let value = unsafe {
                    let edit = GetDlgItem(dialog, ID_EDIT);
                    let len = GetWindowTextLengthW(edit).max(0) as usize;
                    let mut buffer = vec![0u16; len + 1];
                    let read = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize;
                    String::from_utf16_lossy(&buffer[..read])
                };
                TOKEN_TEXT.with(|text| *text.borrow_mut() = value);
                unsafe { EndDialog(dialog, 1) };
                1
            } else if id == IDCANCEL {
                unsafe { EndDialog(dialog, 0) };
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn ask_token(current: &str, parent: HWND) -> String {
    DIALOG_OPEN.set(true);
    TOKEN_TEXT.with(|text| *text.borrow_mut() = current.to_string());
    let template = token_template();
    let result = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(std::ptr::null()),
            template.as_ptr() as *const DLGTEMPLATE,
            parent,
            Some(token_proc),
            0,
        )
    };
    DIALOG_OPEN.set(false);
    let value = TOKEN_TEXT.with(|text| std::mem::take(&mut *text.borrow_mut()));
    if result == 1 {
        value.trim().to_string()
    } else {
        String::new()
    }
}

fn message(text: &str) {
    let text = wide(text);
    let title = wide(APP);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
}

fn on_state() -> OnState {
    Arc::new(|state: &str, detail: &str| {
        info!("tray {}: {}", state, detail);
        {
            let mut shared = shared();
            shared.state = STATES.iter().position(|s| *s == state).unwrap_or(0);
            shared.detail = detail.to_string();
        }
        post(WM_STATE);
    })
}

fn updates() {
    let mut delay = START_DELAY;
    loop {
        let manual = {
            let (lock, wake) = &WAKE;
            let flags = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (mut flags, _) = wake
                .wait_timeout_while(flags, Duration::from_secs(delay), |flags| !flags.0)
                .unwrap_or_else(|e| e.into_inner());
            let manual = flags.1;
            *flags = (false, false);
            manual
        };
        delay = INTERVAL;
        if shared().update == Update::Ready {
            continue;
        }
        check(manual);
    }
}

fn check(manual: bool) {
    let settings = store::read_settings();
    let get = |name: &str, default: &'static str| settings.get(name).cloned().unwrap_or(default.to_string());
    if !manual && get("CHECK_FOR_UPDATES", "true").trim().to_lowercase() != "true" {
        return;
    }
    set_update(Update::Checking);
    let channel = get("UPDATE_CHANNEL", "release").trim().to_lowercase();
    let skipped = if manual {
        String::new()
    } else {
        get("SKIPPED_TAG", "").trim().to_string()
    };
    let result = (|| -> Result<(), String> {
        let updater = Updater::new()?;
        let Some(chosen) = updater.check(&channel, &skipped)? else {
            info!("no update on the {} channel, staying on {}", channel, VERSION);
            set_update(Update::Current);
            if manual {
                notify("You are on the latest version.".into());
            }
            return Ok(());
        };
        let tag = chosen
            .get("tag_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        shared().tag = tag.clone();
        info!("downloading {}", tag);
        let staged = updater.stage(&chosen)?;
        info!("{} is staged and ready to install", tag);
        {
            let mut shared = shared();
            shared.staged = Some(staged);
            shared.update = Update::Ready;
        }
        notify(format!("{} is ready to install.", tag));
        Ok(())
    })();
    if let Err(e) = result {
        error!("update check failed: {}", e);
        set_update(Update::Failed);
        if manual {
            notify("Update check failed, see the log.".into());
        }
    }
}

fn run(config: Config) -> i32 {
    let icons: Vec<HICON> = ICONS.iter().filter_map(|ico| load_icon(ico)).collect();
    ICON_HANDLES.with(|handles| *handles.borrow_mut() = icons);
    shared().detail = "starting".into();
    let Some(hwnd) = create_window() else {
        error!("could not create the tray window");
        return 1;
    };
    WINDOW.store(hwnd as isize, Ordering::SeqCst);
    let taskbar = wide("TaskbarCreated");
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(taskbar.as_ptr()) }, Ordering::SeqCst);
    add_icon(APP);
    let service = Service::start(config, on_state());
    SERVICE.with(|slot| *slot.borrow_mut() = Some(service));
    if update::checkable() {
        update::clean();
        std::thread::Builder::new()
            .name("updates".into())
            .spawn(updates)
            .expect("updates thread");
    }
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    0
}

fn self_test() -> i32 {
    let result = (|| -> Result<(), String> {
        if !store::check_crypto() {
            return Err("DPAPI round trip failed".into());
        }
        for ico in ICONS {
            let icon = load_icon(ico).ok_or("a tray icon did not load")?;
            unsafe { DestroyIcon(icon) };
        }
        let hwnd = create_window().ok_or("the tray window could not be created")?;
        unsafe { DestroyWindow(hwnd) };
        let body = update::render(Path::new("a.exe"), Path::new("b.exe"), 1);
        if body.contains("%(") {
            return Err("the update script template did not render".into());
        }
        if token_template().len() < 32 {
            return Err("the token dialog template is empty".into());
        }
        crate::discord::Http::new("self-test".into()).map_err(|e| e.to_string())?;
        Updater::new()?;
        Ok(())
    })();
    let path = store::path_for("self-test.log");
    match result {
        Ok(()) => {
            let _ = std::fs::write(path, VERSION);
            0
        }
        Err(e) => {
            let _ = std::fs::write(path, e);
            3
        }
    }
}

pub fn main() -> i32 {
    if std::env::args().skip(1).any(|arg| arg == "--self-test") {
        return self_test();
    }
    store::start_logging();
    let Some(_lock) = store::single_instance() else {
        message(&format!("{} is already running.", APP));
        return 0;
    };
    let mut token = store::read_token();
    if token.is_empty() {
        token = ask_token("", std::ptr::null_mut());
        if token.is_empty() {
            return 0;
        }
        if let Err(e) = store::write_token(&token) {
            error!("could not save the token: {}", e);
        }
    }
    let settings = store::read_settings();
    let config = match load_config(&|name| {
        if name == "DISCORD_TOKEN" {
            Some(token.clone())
        } else {
            settings.get(name).cloned()
        }
    }) {
        Ok(config) => config,
        Err(e) => {
            message(&format!("settings.json: {}", e));
            return 2;
        }
    };
    info!(
        "starting {} poll={}s managed={} restore={}",
        VERSION,
        config.poll_seconds,
        py_list(config.managed.iter().map(String::as_str)),
        config.restore
    );
    run(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tray_icon_loads() {
        for ico in ICONS {
            let icon = load_icon(ico).expect("icon");
            unsafe { DestroyIcon(icon) };
        }
        assert!(load_icon(b"junk").is_none());
    }

    #[test]
    fn the_token_template_is_dword_aligned_and_complete() {
        let template = token_template();
        assert_eq!(template[0] & WS_POPUP, WS_POPUP);
        let words: Vec<u16> = template.iter().flat_map(|v| [*v as u16, (*v >> 16) as u16]).collect();
        let text = String::from_utf16_lossy(&words);
        for label in [
            "discord-mobile-idle",
            "Segoe UI",
            "Paste your Discord token",
            "OK",
            "Cancel",
        ] {
            assert!(text.contains(label), "{}", label);
        }
    }

    #[test]
    fn tooltips_and_menu_text_escape_cleanly() {
        assert_eq!(
            tooltip("connecting"),
            format!("discord-mobile-idle {} - connecting", VERSION)
        );
        assert_eq!(String::from_utf16_lossy(&menu_text("a&b")), "a&&b\0");
        let mut tip = [1u16; 4];
        copy_into(&mut tip, "abcdef");
        assert_eq!(tip, [97, 98, 99, 0]);
    }
}
