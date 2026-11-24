//! Vie en arrière-plan : raccourci global Alt+Espace, icône de la zone de notification,
//! démarrage avec Windows (clé Run de l'utilisateur). Un thread à part avec sa boucle de messages :
//! aucun sondage, CPU nul au repos.
use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::Duration;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Ce que le fond demande à la boucle principale.
pub enum Fond {
    /// Raccourci ou clic sur l'icône, avec le texte sélectionné dans l'app au premier plan.
    Afficher { sel: Option<String> },
    /// La sélection est arrivée après l'ouverture de la barre.
    Selection(String),
    Quitter,
}

type Sink = Box<dyn Fn(Fond) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
/// Notre propre fenêtre (la barre) : le raccourci pressé depuis elle ne lit pas de sélection.
pub static BARRE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

const WM_TRAY: u32 = WM_APP + 1;
const ID_HOTKEY: i32 = 1;
const CMD_OUVRIR: usize = 1;
const CMD_DEMARRAGE: usize = 2;
const CMD_QUITTER: usize = 3;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Lance le thread du fond. Renvoie une erreur lisible si Alt+Espace est déjà pris.
pub fn start(sink: impl Fn(Fond) + Send + Sync + 'static) -> std::sync::mpsc::Receiver<Result<(), String>> {
    let _ = SINK.set(Box::new(sink));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("fond".into())
        .spawn(move || unsafe { run(tx) })
        .ok();
    rx
}

fn send(f: Fond) {
    if let Some(s) = SINK.get() {
        s(f);
    }
}

unsafe fn run(tx: std::sync::mpsc::Sender<Result<(), String>>) {
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("LanceurFond");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
        if hwnd.is_null() {
            let _ = tx.send(Err("fenêtre de fond impossible".into()));
            return;
        }
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w("TaskbarCreated").as_ptr()));
        tray(hwnd, NIM_ADD);
        let ok = RegisterHotKey(hwnd, ID_HOTKEY, MOD_ALT | MOD_NOREPEAT, VK_SPACE as u32) != 0;
        let _ = tx.send(if ok { Ok(()) } else { Err("Alt+Espace est déjà pris par une autre app".into()) });
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn tray(hwnd: HWND, op: u32) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        // Icône n° 1 de l'exe (posée par winresource), sinon l'icône générique.
        nid.hIcon = LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as _);
        if nid.hIcon.is_null() {
            nid.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        for (d, s) in nid.szTip.iter_mut().zip("Lanceur (Alt+Espace)".encode_utf16()) {
            *d = s;
        }
        Shell_NotifyIconW(op, &nid);
    }
}

/// Lit la sélection de l'app au premier plan (UI Automation) en laissant au plus 35 ms
/// avant d'ouvrir la barre : après, la barre a le focus et la sélection n'est plus lisible.
fn afficher() {
    let fg = unsafe { GetForegroundWindow() } as isize;
    if fg == 0 || fg == BARRE.load(std::sync::atomic::Ordering::Relaxed) {
        send(Fond::Afficher { sel: None });
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let s = crate::uia::selected_text();
        let _ = tx.send(s);
    });
    match rx.recv_timeout(Duration::from_millis(35)) {
        Ok(sel) => send(Fond::Afficher { sel }),
        Err(_) => {
            send(Fond::Afficher { sel: None });
            std::thread::spawn(move || {
                if let Ok(Some(s)) = rx.recv_timeout(Duration::from_millis(400)) {
                    send(Fond::Selection(s));
                }
            });
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if Some(&msg) == TASKBAR_CREATED.get() {
            tray(hwnd, NIM_ADD); // Explorer a redémarré : on remet l'icône
            return 0;
        }
        match msg {
            WM_HOTKEY if wp as i32 == ID_HOTKEY => {
                afficher();
                0
            }
            WM_TRAY => {
                match lp as u32 {
                    WM_LBUTTONUP => send(Fond::Afficher { sel: None }),
                    WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                tray(hwnd, NIM_DELETE);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn menu(hwnd: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, CMD_OUVRIR, w("Ouvrir (Alt+Espace)").as_ptr());
        let chk = if autostart_on() { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(m, MF_STRING | chk, CMD_DEMARRAGE, w("Démarrer avec Windows").as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(m, MF_STRING, CMD_QUITTER, w("Quitter").as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd); // exigé pour que le menu se ferme en cliquant ailleurs
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null()) as usize;
        DestroyMenu(m);
        match cmd {
            CMD_OUVRIR => send(Fond::Afficher { sel: None }),
            CMD_DEMARRAGE => {
                let on = !autostart_on();
                let _ = autostart_set(on);
                crate::app::set_demarrage(on);
            }
            CMD_QUITTER => send(Fond::Quitter),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Démarrage avec Windows : HKCU\Software\Microsoft\Windows\CurrentVersion\Run, valeur « Lanceur ».
// ---------------------------------------------------------------------------------------------

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn run_value() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --fond", exe.display()))
}

fn read_run() -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, w(RUN).as_ptr(), w("Lanceur").as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut len);
        if r != 0 {
            return None;
        }
        let n = (len as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
    }
}

pub fn autostart_on() -> bool {
    read_run().is_some()
}

/// Écrit ou retire l'entrée Run. Ne réécrit que si la valeur change (exe déplacé).
pub fn autostart_set(on: bool) -> Result<(), String> {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w(RUN).as_ptr(), 0, KEY_SET_VALUE | KEY_QUERY_VALUE, &mut key) != 0 {
            return Err("clé Run inaccessible".into());
        }
        let r = if on {
            let v = run_value().ok_or("chemin de l'exe inconnu")?;
            if read_run().as_deref() == Some(v.as_str()) {
                0
            } else {
                let data = w(&v);
                RegSetValueExW(key, w("Lanceur").as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32)
            }
        } else {
            let r = RegDeleteValueW(key, w("Lanceur").as_ptr());
            if r == ERROR_FILE_NOT_FOUND { 0 } else { r }
        };
        RegCloseKey(key);
        if r == 0 { Ok(()) } else { Err(format!("registre : erreur {r}")) }
    }
}
