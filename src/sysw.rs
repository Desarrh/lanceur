//! sysw : petits appels système Win32 pour Lanceur.
use std::ffi::{OsStr, c_void};
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Com::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Power::SetSuspendState;
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CF_UNICODETEXT: u32 = 13;

pub struct Listener {
    pub pid: u32,
    pub exe: Option<PathBuf>,
    /// Nom de fichier de l'exe, ou "pid 1234".
    pub name: String,
}

/// Chaîne UTF-16 terminée par NUL.
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Poignée Win32 fermée automatiquement.
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Lit la table TCP « listener » d'une famille ; renvoie (port, pid) de chaque ligne.
fn tcp_table(af: u32) -> Vec<(u16, u32)> {
    let mut size = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    // Deux passes (taille puis données), quelques essais si la table grossit entre-temps.
    for _ in 0..5 {
        let p = if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr() as *mut c_void };
        let r = unsafe { GetExtendedTcpTable(p, &mut size, 0, af, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        match r {
            0 => break,
            ERROR_INSUFFICIENT_BUFFER => buf.resize((size as usize).div_ceil(4), 0),
            _ => return Vec::new(),
        }
    }
    if buf.is_empty() {
        return Vec::new();
    }
    let n = buf[0] as usize;
    let rows = unsafe { buf.as_ptr().add(1) };
    let mut out = Vec::new();
    for i in 0..n {
        // Le port est en ordre réseau dans les 16 bits bas.
        if af == AF_INET as u32 {
            let r = unsafe { std::ptr::read_unaligned((rows as *const MIB_TCPROW_OWNER_PID).add(i)) };
            out.push((u16::from_be(r.dwLocalPort as u16), r.dwOwningPid));
        } else {
            let r = unsafe { std::ptr::read_unaligned((rows as *const MIB_TCP6ROW_OWNER_PID).add(i)) };
            out.push((u16::from_be(r.dwLocalPort as u16), r.dwOwningPid));
        }
    }
    out
}

/// Processus en écoute TCP (IPv4 et IPv6) sur ce port, dédoublonnés par pid.
pub fn listeners(port: u16) -> Vec<Listener> {
    let mut v: Vec<Listener> = Vec::new();
    for af in [AF_INET as u32, AF_INET6 as u32] {
        for (p, pid) in tcp_table(af) {
            if p == port && !v.iter().any(|l| l.pid == pid) {
                let exe = exe_of(pid);
                let name = exe
                    .as_ref()
                    .and_then(|e| e.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("pid {pid}"));
                v.push(Listener { pid, exe, name });
            }
        }
    }
    v
}

/// Chemin de l'exe d'un processus déjà ouvert.
fn exe_of_handle(h: HANDLE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Chemin complet de l'exe d'un processus.
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let h = Handle(h);
    exe_of_handle(h.0)
}

/// Comparaison de chemins insensible à la casse (Windows).
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Tue le processus seulement si son exe est toujours `expected` (les pid sont réutilisés).
pub fn kill(pid: u32, expected: &Path) -> Result<(), String> {
    let h = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(format!("Impossible d'ouvrir le processus {pid} (droits insuffisants ou déjà terminé)."));
    }
    let h = Handle(h);
    // Vérification faite sur la même poignée : pas de course avec une réutilisation du pid.
    match exe_of_handle(h.0) {
        Some(e) if same_path(&e, expected) => {}
        Some(e) => return Err(format!("Le processus {pid} n'est plus {} (c'est maintenant {}) : arrêt refusé.", expected.display(), e.display())),
        None => return Err(format!("Chemin du processus {pid} illisible : arrêt refusé.")),
    }
    if unsafe { TerminateProcess(h.0, 1) } == 0 {
        return Err(format!("Échec de l'arrêt du processus {pid} (erreur {}).", unsafe { GetLastError() }));
    }
    Ok(())
}

pub fn lock() -> Result<(), String> {
    if unsafe { LockWorkStation() } == 0 {
        return Err(format!("Impossible de verrouiller la session (erreur {}).", unsafe { GetLastError() }));
    }
    Ok(())
}

pub fn sleep() -> Result<(), String> {
    if !unsafe { SetSuspendState(false, false, false) } {
        return Err(format!("Impossible de mettre en veille (erreur {}).", unsafe { GetLastError() }));
    }
    Ok(())
}

/// Vide la corbeille ; la confirmation Windows est conservée, seul le son est coupé.
pub fn empty_recycle_bin() -> Result<(), String> {
    let hr = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), SHERB_NOSOUND) };
    if hr < 0 {
        return Err(format!("Impossible de vider la corbeille (HRESULT 0x{:08X}).", hr as u32));
    }
    Ok(())
}

/// (octets, éléments) de la corbeille.
#[cfg_attr(not(test), allow(dead_code))]
pub fn recycle_bin_size() -> Option<(u64, u64)> {
    let mut i = SHQUERYRBINFO { cbSize: size_of::<SHQUERYRBINFO>() as u32, i64Size: 0, i64NumItems: 0 };
    let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut i) };
    (hr >= 0).then(|| (i.i64Size.max(0) as u64, i.i64NumItems.max(0) as u64))
}

/// Ouvre une cible (fichier, dossier, URL, .lnk, shell:...) via le verbe « open ».
pub fn open(target: &str, args: Option<&str>, cwd: Option<&Path>) -> Result<(), String> {
    let t = wide(OsStr::new(target));
    let a = args.map(|s| wide(OsStr::new(s)));
    let d = cwd.map(|p| wide(p.as_os_str()));
    let verb = wide(OsStr::new("open"));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = t.as_ptr();
    info.lpParameters = a.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    info.lpDirectory = d.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
    info.nShow = 1; // SW_SHOWNORMAL
    // Le shell préfère un thread COM initialisé ; un échec (déjà initialisé autrement) est sans gravité.
    unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(format!("Impossible d'ouvrir « {target} » (erreur {}).", unsafe { GetLastError() }));
    }
    Ok(())
}

/// Ouvre l'Explorateur avec le fichier sélectionné (via `explorer.exe /select,"chemin"`).
pub fn reveal(path: &Path) -> Result<(), String> {
    // shortcut: pas de SHOpenFolderAndSelectItems, l'appel explorer suffit.
    // explorer exige les guillemets littéraux, d'où raw_arg.
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Impossible d'ouvrir l'Explorateur sur « {} » : {e}.", path.display()))
}

/// Lance un exe sans console, récupère stdout avec un délai maximal (l'enfant est tué à l'expiration).
pub fn run_capture(exe: &Path, args: &[&str], timeout: Duration) -> Result<(i32, String), String> {
    let mut child = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Impossible de lancer {} : {e}.", exe.display()))?;
    let mut out = child.stdout.take().ok_or("Sortie standard indisponible.")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        let _ = tx.send(b);
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Délai dépassé ({} ms) pour {}.", timeout.as_millis(), exe.display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("Attente de {} impossible : {e}.", exe.display())),
        }
    };
    // Court délai : un petit-enfant pourrait garder le tube ouvert.
    let bytes = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    Ok((status.code().unwrap_or(-1), String::from_utf8_lossy(&bytes).into_owned()))
}

/// Copie un texte dans le presse-papiers (CF_UNICODETEXT).
pub fn set_clipboard(text: &str) -> Result<(), String> {
    // Le presse-papiers peut être tenu par un autre processus : quelques essais.
    let mut ouvert = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            ouvert = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if !ouvert {
        return Err("Presse-papiers occupé par un autre programme.".into());
    }
    let w = wide(OsStr::new(text));
    let bytes = w.len() * 2;
    let res = unsafe {
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h.is_null() {
            Err("Mémoire insuffisante pour le presse-papiers.".to_string())
        } else {
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                GlobalFree(h);
                Err("Verrouillage mémoire impossible.".to_string())
            } else {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(h);
                // En cas de succès, le système possède la mémoire ; sinon on la libère.
                if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                    GlobalFree(h);
                    Err(format!("Écriture dans le presse-papiers impossible (erreur {}).", GetLastError()))
                } else {
                    Ok(())
                }
            }
        }
    };
    unsafe { CloseClipboard() };
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> PathBuf {
        PathBuf::from(r"C:\Windows\System32\ping.exe")
    }

    /// Enfant inoffensif et invisible (≈ 30 s).
    fn enfant() -> std::process::Child {
        Command::new(cache())
            .args(["-n", "30", "127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ping")
    }

    #[test]
    fn ecoute() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = l.local_addr().expect("addr").port();
        let moi = std::process::id();
        let v = listeners(port);
        let e = v.iter().find(|x| x.pid == moi).expect("pid courant absent");
        let cur = std::env::current_exe().expect("exe");
        assert!(same_path(e.exe.as_ref().expect("exe None"), &cur));
        assert_eq!(e.name.to_lowercase(), cur.file_name().expect("nom").to_string_lossy().to_lowercase());
    }

    #[test]
    fn exe_courant() {
        let cur = std::env::current_exe().expect("exe");
        assert!(same_path(&exe_of(std::process::id()).expect("exe_of"), &cur));
    }

    #[test]
    fn kill_garde_fou() {
        let mut c = enfant();
        let pid = c.id();
        assert!(kill(pid, Path::new(r"C:\autre\chose.exe")).is_err());
        assert!(c.try_wait().expect("try_wait").is_none(), "l'enfant devait survivre");
        kill(pid, &cache()).expect("kill");
        c.wait().expect("wait");
    }

    #[test]
    fn capture() {
        let cmd = PathBuf::from(r"C:\Windows\System32\cmd.exe");
        let (code, out) = run_capture(&cmd, &["/c", "echo bonjour"], Duration::from_secs(5)).expect("run");
        assert_eq!(code, 0);
        assert_eq!(out.trim(), "bonjour");
    }

    #[test]
    fn capture_delai() {
        let t = Instant::now();
        let r = run_capture(&cache(), &["-n", "10", "127.0.0.1"], Duration::from_millis(300));
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn corbeille_lecture() {
        assert!(recycle_bin_size().is_some());
    }
}
