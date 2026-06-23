//! Win32 window helpers: enumerate windows, read class/title/pid, and "wake"
//! Chromium/WebView2 accessibility via WM_GETOBJECT.

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VIRTUAL_KEY,
    VK_CONTROL, VK_RETURN,
};
use windows::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SendMessageW, SetCursorPos, SetForegroundWindow,
    SetWindowPos, ShowWindow, SystemParametersInfoW, SPI_GETWORKAREA,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_RESTORE, WM_GETOBJECT,
};

/// Hide the console window, but only if this process owns it alone (i.e. launched by
/// double-click). When run from a terminal the console is shared, so we leave it.
pub fn hide_console_if_owned() {
    unsafe {
        let mut buf = [0u32; 8];
        let count = GetConsoleProcessList(&mut buf);
        if count == 1 {
            let hwnd = GetConsoleWindow();
            if !hwnd.is_invalid() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

/// UIA root object id — the magic lParam that forces a Chromium/WebView2 renderer
/// (e.g. Copilot for Windows) to build its UI Automation tree. -4 (OBJID_CLIENT)
/// works for many Electron apps; -25 (UiaRootObjectId) is needed for the stubborn ones.
const UIA_ROOT_OBJECT_ID: isize = -25;
const OBJID_CLIENT: isize = -4;

unsafe extern "system" fn collect_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<HWND>);
    v.push(hwnd);
    BOOL(1)
}

pub fn enum_top_windows() -> Vec<HWND> {
    let mut v: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

pub fn enum_child_windows(parent: HWND) -> Vec<HWND> {
    let mut v: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(parent, Some(collect_cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

pub fn is_visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

pub fn restore_and_foreground(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// The window that currently has user focus (so we can hand it back after driving Copilot).
pub fn get_foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// Make `hwnd` the foreground window WITHOUT moving or restoring it.
pub fn set_foreground(hwnd: HWND) {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Bring `hwnd` to a visible position (x, y) and focus it. Used to pull Copilot back on screen
/// for the brief moment we type into it (and by `copilot-show` if it ever gets parked).
pub fn move_onscreen(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Top-left screen position of `hwnd`.
pub fn window_xy(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        (r.left, r.top)
    }
}

/// Primary monitor work area (screen minus taskbar): (left, top, width, height).
pub fn work_area() -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut r as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// Position AND size a window without changing focus or z-order. Used to tuck Copilot into a
/// small corner so it stays visible (it must, to keep generating) without hogging the screen.
pub fn place_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetWindowPos(hwnd, HWND::default(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Find the first visible top-level window whose title contains `substr`.
pub fn find_visible_window_by_title(substr: &str) -> Option<HWND> {
    enum_top_windows()
        .into_iter()
        .find(|&h| is_visible(h) && window_text(h).contains(substr))
}

/// Force the Chromium/WebView2 accessibility tree to build for `hwnd` and all of
/// its descendant windows, by posting WM_GETOBJECT with both magic object ids.
pub fn wake_accessibility(hwnd: HWND) {
    let mut targets = vec![hwnd];
    targets.extend(enum_child_windows(hwnd));
    for t in targets {
        unsafe {
            SendMessageW(t, WM_GETOBJECT, WPARAM(0), LPARAM(UIA_ROOT_OBJECT_ID));
            SendMessageW(t, WM_GETOBJECT, WPARAM(0), LPARAM(OBJID_CLIENT));
        }
    }
}

/// Type Unicode text into the currently focused control via real keystrokes.
/// Needed for WebView2/React inputs (e.g. Copilot) that ignore ValuePattern.SetValue.
pub fn type_unicode(text: &str) {
    let unit = |scan: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // Inject in small chunks with pauses — a single huge burst is dropped by
    // Chromium/React inputs (e.g. Copilot) and the message ends up incomplete.
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(24) {
        let mut inputs: Vec<INPUT> = Vec::with_capacity(chunk.len() * 2);
        for &u in chunk {
            inputs.push(unit(u, false));
            inputs.push(unit(u, true));
        }
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

fn vkey(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Press the Enter key (used to submit a chat prompt to the focused input). The key is held
/// briefly between down and up: Copilot's WebView2 input drops a zero-duration Enter and the
/// message never submits.
pub fn press_enter() {
    unsafe {
        SendInput(&[vkey(VK_RETURN, false)], std::mem::size_of::<INPUT>() as i32);
        std::thread::sleep(std::time::Duration::from_millis(60));
        SendInput(&[vkey(VK_RETURN, true)], std::mem::size_of::<INPUT>() as i32);
    }
}

/// Put UTF-16 text on the clipboard (for fast, reliable paste of long content).
pub fn set_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    unsafe {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * std::mem::size_of::<u16>();
        let hg: HGLOBAL = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let ptr = GlobalLock(hg);
        if ptr.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
        let _ = GlobalUnlock(hg);
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(13u32, HANDLE(hg.0)).is_ok(); // 13 = CF_UNICODETEXT
        let _ = CloseClipboard();
        ok
    }
}

/// Paste (Ctrl+V) into the focused control.
pub fn paste() {
    unsafe {
        let v = VIRTUAL_KEY(0x56); // 'V'
        let inputs = [
            vkey(VK_CONTROL, false),
            vkey(v, false),
            vkey(v, true),
            vkey(VK_CONTROL, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Move the cursor to (x, y) in virtual-screen coords and left-click.
pub fn click(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
        std::thread::sleep(std::time::Duration::from_millis(90));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        std::thread::sleep(std::time::Duration::from_millis(45));
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

/// Select all text in the focused input (Ctrl+A), so the next typing replaces it.
pub fn select_all() {
    unsafe {
        let a = VIRTUAL_KEY(0x41); // 'A'
        let inputs = [
            vkey(VK_CONTROL, false),
            vkey(a, false),
            vkey(a, true),
            vkey(VK_CONTROL, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Render-host child windows (where the web content's UIA tree usually lives).
pub fn render_host_children(hwnd: HWND) -> Vec<HWND> {
    enum_child_windows(hwnd)
        .into_iter()
        .filter(|&c| class_name(c).contains("RenderWidgetHost"))
        .collect()
}
