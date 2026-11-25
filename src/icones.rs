//! Icônes Windows et jaquettes de jeux, rendues en pixels RGBA (sans egui).
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Com::{CoInitializeEx, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::PCWSTR;

/// Image RGBA (non prémultipliée), ligne par ligne.
pub struct Img { pub w: u32, pub h: u32, pub rgba: Vec<u8> }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// COM sur le thread courant ; un thread déjà initialisé autrement (RPC_E_CHANGED_MODE) n'est pas gênant.
fn com() {
    // SAFETY: simple initialisation COM du thread appelant, résultat volontairement ignoré.
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
}

/// HBITMAP → BGRA brut (GetDIBits top-down 32 bits).
fn bitmap_vers_img(hbm: HBITMAP) -> Option<Img> {
    // SAFETY: appels GDI sur un HBITMAP valide ; les tampons ont la taille annoncée.
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbm.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 { return None; }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 { return None; }
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h,
            biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let n = GetDIBits(dc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if n == 0 { return None; }
        Some(Img { w: w as u32, h: h as u32, rgba: buf })
    }
}

/// BGRA (éventuellement prémultiplié) → RGBA droit. Sans aucun alpha : opaque.
fn bgra_vers_rgba(img: &mut Img) {
    let opaque = img.rgba.chunks_exact(4).all(|p| p[3] == 0);
    for p in img.rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
        if opaque { p[3] = 255; } else if p[3] != 0 && p[3] != 255 {
            let a = p[3] as u32;
            for c in &mut p[..3] { *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8; }
        }
    }
}

/// HICON → Img (bitmap couleur ; si pas d'alpha, on déduit la transparence du masque).
fn hicon_vers_img(hi: HICON) -> Option<Img> {
    // SAFETY: GetIconInfo remplit deux bitmaps qu'on libère ; l'icône est détruite par l'appelant.
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(hi, &mut ii).ok()?;
        let couleur = bitmap_vers_img(ii.hbmColor);
        let masque = if ii.hbmMask.is_invalid() { None } else { bitmap_vers_img(ii.hbmMask) };
        let _ = DeleteObject(ii.hbmColor.into());
        let _ = DeleteObject(ii.hbmMask.into());
        let mut img = couleur?;
        if img.rgba.chunks_exact(4).all(|p| p[3] == 0) {
            if let Some(m) = masque.filter(|m| m.rgba.len() == img.rgba.len()) {
                for (p, q) in img.rgba.chunks_exact_mut(4).zip(m.rgba.chunks_exact(4)) {
                    p[3] = if q[0] == 0 { 255 } else { 0 };
                }
            }
        }
        bgra_vers_rgba(&mut img);
        Some(img)
    }
}

/// Cible d'un .lnk (pour éviter la flèche de raccourci), si elle existe sur disque.
fn cible_lnk(chemin: &str) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
    // SAFETY: objets COM créés et utilisés sur le thread courant, tampon de 520 u16.
    unsafe {
        let lien: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = windows::core::Interface::cast(&lien).ok()?;
        pf.Load(PCWSTR(wide(chemin).as_ptr()), STGM_READ).ok()?;
        let _ = lien.Resolve(HWND::default(), (SLR_NO_UI.0 | SLR_NOSEARCH.0) as u32);
        let mut buf = [0u16; 520];
        let mut fd = WIN32_FIND_DATAW::default();
        lien.GetPath(&mut buf, &mut fd, 0).ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let cible = String::from_utf16(&buf[..n]).ok()?;
        (!cible.is_empty() && Path::new(&cible).exists()).then_some(cible)
    }
}

/// Icône Windows d'un chemin (exe, .lnk — icône de la cible —, dossier, fichier), à `taille` px, avec alpha.
/// Appelé depuis un thread de fond. None si échec.
pub fn icone(chemin: &str, taille: u32) -> Option<Img> {
    com();
    let cible = if chemin.to_lowercase().ends_with(".lnk") { cible_lnk(chemin) } else { None };
    let chemin = cible.as_deref().unwrap_or(chemin);
    via_factory(chemin, taille).or_else(|| via_shgetfileinfo(chemin))
}

fn via_factory(chemin: &str, taille: u32) -> Option<Img> {
    // SAFETY: le HBITMAP renvoyé nous appartient, on le libère après lecture.
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wide(chemin).as_ptr()), None::<&IBindCtx>).ok()?;
        let hbm = f.GetImage(SIZE { cx: taille as i32, cy: taille as i32 }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let img = bitmap_vers_img(hbm);
        let _ = DeleteObject(hbm.into());
        let mut img = img?;
        bgra_vers_rgba(&mut img);
        Some(img)
    }
}

fn via_shgetfileinfo(chemin: &str) -> Option<Img> {
    // SAFETY: SHFILEINFOW local ; l'HICON obtenu est détruit après conversion.
    unsafe {
        let mut sfi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(PCWSTR(wide(chemin).as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut sfi), size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON);
        if r == 0 || sfi.hIcon.is_invalid() { return None; }
        let img = hicon_vers_img(sfi.hIcon);
        let _ = DestroyIcon(sfi.hIcon);
        img
    }
}

/// Icône générique d'une extension (".txt"…) sans toucher au disque ; `dossier` → icône de dossier.
pub fn icone_type(ext: &str, dossier: bool, taille: u32) -> Option<Img> {
    com();
    let nom = if dossier { "dossier".to_string() } else { format!("x.{}", ext.trim_start_matches('.')) };
    let attr = if dossier { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    // SAFETY: SHGetFileInfoW sans accès disque, puis liste d'images système ; HICON détruit après usage.
    unsafe {
        let mut sfi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(PCWSTR(wide(&nom).as_ptr()), attr, Some(&mut sfi), size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES);
        if r == 0 { return None; }
        // SHIL_LARGE = 0 (32 px), SHIL_EXTRALARGE = 2 (48 px)
        let liste: IImageList = SHGetImageList(if taille > 32 { 2 } else { 0 }).ok()?;
        let hi = liste.GetIcon(sfi.iIcon, ILD_TRANSPARENT.0).ok()?;
        let img = hicon_vers_img(hi);
        let _ = DestroyIcon(hi);
        img
    }
}

/// Faut-il une icône par fichier (exe, lnk…) plutôt que par extension ?
pub fn icone_propre(chemin: &str) -> bool {
    let ext = Path::new(chemin).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    ["exe", "lnk", "ico", "url", "appref-ms", "msc", "cpl"].contains(&ext.as_str())
}

/// Jaquette réduite à `w`×`h` (rognée au centre) : cache, puis cache Steam, puis téléchargement https.
pub fn jaquette(cle: &str, url: Option<&str>, steam: Option<&Path>, steam_appid: Option<u64>, cache: &Path, w: u32, h: u32) -> Option<Img> {
    if cle.is_empty() || cle.contains(['/', '\\', ':']) || cle.contains("..") || w == 0 || h == 0 { return None; }
    let png = cache.join(format!("{cle}.png"));
    if let Ok(i) = image::open(&png) {
        let i = i.to_rgba8();
        if i.dimensions() == (w, h) { return Some(Img { w, h, rgba: i.into_raw() }); }
    }
    let src = steam_local(steam, steam_appid).or_else(|| telecharger(url?, cache, cle))?;
    let petit = src.resize_to_fill(w, h, image::imageops::FilterType::Triangle);
    // Écriture atomique : .tmp puis rename. Un échec d'écriture n'empêche pas de renvoyer l'image.
    let tmp = cache.join(format!("{cle}.png.tmp"));
    if std::fs::create_dir_all(cache).is_ok()
        && petit.save_with_format(&tmp, image::ImageFormat::Png).is_ok()
        && std::fs::rename(&tmp, &png).is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
    let rgba = petit.to_rgba8();
    Some(Img { w: rgba.width(), h: rgba.height(), rgba: rgba.into_raw() })
}

/// Jaquette 600x900 dans le cache de bibliothèque Steam.
fn steam_local(steam: Option<&Path>, appid: Option<u64>) -> Option<image::DynamicImage> {
    let (dossier, id) = (steam?.join("appcache").join("librarycache"), appid?);
    let sous = dossier.join(id.to_string());
    let mut cand: Vec<PathBuf> = vec![dossier.join(format!("{id}_library_600x900.jpg")), sous.join("library_600x900.jpg")];
    // .jpg "600x900" du sous-dossier <id>\ (et un niveau en dessous : dossiers de hachage)
    for niveau in 0..2 {
        let racines: Vec<PathBuf> = if niveau == 0 { vec![sous.clone()] } else {
            std::fs::read_dir(&sous).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
        };
        for d in racines {
            for p in std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.path()) {
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg"))
                    && p.file_name().is_some_and(|n| n.to_string_lossy().contains("600x900")) { cand.push(p); }
            }
        }
    }
    cand.into_iter().find_map(|p| image::open(p).ok())
}

/// Téléchargement via curl.exe (https seulement, ≤ 5 Mo, sans console).
fn telecharger(url: &str, cache: &Path, cle: &str) -> Option<image::DynamicImage> {
    use std::os::windows::process::CommandExt;
    if !url.starts_with("https://") { return None; }
    std::fs::create_dir_all(cache).ok()?;
    let tmp = cache.join(format!("{cle}.dl.tmp"));
    let curl = Path::new(&std::env::var_os("SystemRoot")?).join("System32").join("curl.exe");
    let ok = std::process::Command::new(curl)
        .args(["-sSfL", "--max-time", "8", "--max-filesize", "5242880", "--proto", "=https", "-o"])
        .arg(&tmp).arg("--").arg(url)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status().is_ok_and(|s| s.success());
    let octets = if ok { std::fs::metadata(&tmp).ok().filter(|m| m.len() <= 5 << 20).and_then(|_| std::fs::read(&tmp).ok()) } else { None };
    let _ = std::fs::remove_file(&tmp);
    image::load_from_memory(&octets?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icones_windows() {
        let i = icone(r"C:\Windows\System32\notepad.exe", 32).expect("notepad");
        assert!(i.w >= 32 && i.h >= 32 && i.rgba.len() == (i.w * i.h * 4) as usize);
        assert!(i.rgba.chunks_exact(4).any(|p| p[3] > 0));
        assert!(icone_type(".txt", false, 32).is_some());
        assert!(icone_type("", true, 32).is_some());
        assert!(icone_type(".pdf", false, 48).is_some());
        assert!(icone_propre("a.EXE") && !icone_propre("a.txt"));
    }

    #[test]
    fn jaquette_cache() {
        let base = std::env::temp_dir().join(format!("lanceur_icones_jaq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (steam, cache) = (base.join("steam"), base.join("cache"));
        let lc = steam.join("appcache").join("librarycache");
        std::fs::create_dir_all(&lc).expect("mkdir");
        let img = image::RgbImage::from_fn(600, 900, |x, y| image::Rgb([(x / 3) as u8, (y / 4) as u8, 90]));
        img.save(lc.join("570_library_600x900.jpg")).expect("jpg");
        let j = jaquette("steam-570", None, Some(&steam), Some(570), &cache, 40, 60).expect("steam");
        assert_eq!((j.w, j.h), (40, 60));
        assert!(cache.join("steam-570.png").exists());
        std::fs::remove_dir_all(&steam).expect("rm steam");
        let j2 = jaquette("steam-570", None, Some(&steam), Some(570), &cache, 40, 60).expect("cache");
        assert_eq!(j.rgba, j2.rgba);
        assert!(jaquette("absent", None, None, None, &cache, 40, 60).is_none());
        assert!(jaquette("x", Some("http://exemple.invalid/a.jpg"), None, None, &cache, 40, 60).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
