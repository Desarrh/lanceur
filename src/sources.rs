//! Ce qu'on peut lancer : apps du menu Démarrer, apps maison (`C:\Dev\*\dist`), jeux, projets,
//! fichiers et dossiers. Tout est lu sur un thread de fond ; rien ici ne touche à l'interface.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Genre {
    App,
    Maison,
    Jeu,
    Commande,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub genre: Genre,
    pub nom: String,
    /// Chemin, URL ou identifiant de commande : sert aussi de clé pour l'apprentissage.
    pub cible: String,
    pub detail: String,
}

/// Dossier des projets de dev.
pub fn racine_dev() -> PathBuf {
    std::env::var_os("LANCEUR_DEV").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Dev"))
}

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from)
}

/// Apps (menu Démarrer + apps maison) et jeux, dédoublonnés par nom.
pub fn items() -> Vec<Item> {
    let mut v = Vec::new();
    jeux(&mut v); // avant le menu Démarrer : le jeu gagne sur son raccourci
    maison(&mut v);
    for d in [env("APPDATA").map(|a| a.join("Microsoft\\Windows\\Start Menu\\Programs")), env("ProgramData").map(|a| a.join("Microsoft\\Windows\\Start Menu\\Programs"))].into_iter().flatten() {
        menu_demarrer(&d, 0, &mut v);
    }
    let mut vus = std::collections::HashSet::new();
    v.retain(|i| vus.insert(socle::fuzzy::fold(&i.nom)));
    v
}

fn menu_demarrer(d: &Path, depth: u32, v: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                menu_demarrer(&p, depth + 1, v);
            }
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        // Les .url du menu Démarrer sont des raccourcis de jeux Steam/Epic, déjà lus à la source.
        if ext != "lnk" {
            continue;
        }
        let nom = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let l = nom.to_lowercase();
        if nom.is_empty() || ["uninstall", "désinstaller", "desinstaller", "readme", "lisez", "help", "aide"].iter().any(|m| l.contains(m)) {
            continue;
        }
        let dossier = p.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or("").to_string();
        let detail = if dossier == "Programs" { "App".into() } else { format!("App · {dossier}") };
        v.push(Item { genre: Genre::App, nom, cible: p.to_string_lossy().into_owned(), detail });
    }
}

/// Apps Rust de l'utilisateur : `C:\Dev\*\dist\*.exe` (sans les anciennes versions), plus Ultimate.
fn maison(v: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(racine_dev()) else { return };
    for e in rd.flatten() {
        let dist = e.path().join("dist");
        let Ok(rd) = std::fs::read_dir(&dist) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            let nom = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let ext_ok = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"));
            if !ext_ok || nom.contains("ancien") || nom.contains('.') {
                continue;
            }
            v.push(Item { genre: Genre::Maison, nom, cible: p.to_string_lossy().into_owned(), detail: "App maison".into() });
        }
    }
    let ult = racine_dev().join("Launcher\\src-tauri\\target\\release\\Ultimate.exe");
    if ult.is_file() {
        v.push(Item { genre: Genre::Maison, nom: "Ultimate".into(), cible: ult.to_string_lossy().into_owned(), detail: "App maison · jeux".into() });
    }
}

// ---------------------------------------------------------------------------------------------
// Jeux : bibliothèque d'Ultimate si elle existe, sinon lecture directe de Steam et Epic.
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JeuUltimate {
    #[serde(default)]
    name: String,
    #[serde(default)]
    launch: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    installed: bool,
}

fn jeux(v: &mut Vec<Item>) {
    let lib = env("APPDATA").map(|a| a.join("dev.local.launcher\\library.json"));
    if let Some(Ok(b)) = lib.map(std::fs::read) {
        if let Ok(js) = serde_json::from_slice::<Vec<JeuUltimate>>(&b) {
            for j in js.into_iter().filter(|j| j.installed && !j.launch.is_empty() && !j.name.is_empty()) {
                v.push(Item { genre: Genre::Jeu, nom: j.name, cible: j.launch, detail: format!("Jeu · {}", source(&j.source)) });
            }
            return;
        }
    }
    steam(v);
    epic(v);
}

fn source(s: &str) -> &str {
    match s {
        "steam" => "Steam",
        "epic" => "Epic",
        "gog" => "GOG",
        _ => s,
    }
}

/// Valeur `"cle"   "valeur"` d'un fichier VDF/ACF (format texte de Steam).
fn vdf<'a>(txt: &'a str, cle: &str) -> impl Iterator<Item = String> + 'a {
    let pat = format!("\"{cle}\"");
    txt.lines().filter_map(move |l| {
        let l = l.trim();
        let rest = l.strip_prefix(&pat)?.trim();
        Some(rest.trim_matches('"').replace("\\\\", "\\"))
    })
}

fn steam(v: &mut Vec<Item>) {
    let racine = steam_path().unwrap_or_else(|| PathBuf::from("C:\\Program Files (x86)\\Steam"));
    let mut libs = vec![racine.clone()];
    if let Ok(t) = std::fs::read_to_string(racine.join("steamapps\\libraryfolders.vdf")) {
        libs.extend(vdf(&t, "path").map(PathBuf::from));
    }
    libs.dedup();
    for lib in libs {
        let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
            let (Some(id), Some(nom)) = (vdf(&t, "appid").next(), vdf(&t, "name").next()) else { continue };
            if nom.contains("Steamworks") || nom.contains("Redistributable") {
                continue;
            }
            v.push(Item { genre: Genre::Jeu, nom, cible: format!("steam://rungameid/{id}"), detail: "Jeu · Steam".into() });
        }
    }
}

fn steam_path() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::*;
    let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let mut buf = [0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, w("Software\\Valve\\Steam").as_ptr(), w("SteamPath").as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut len) };
    if r != 0 {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1).min(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n]).replace('/', "\\")))
}

fn epic(v: &mut Vec<Item>) {
    let Some(d) = env("ProgramData").map(|p| p.join("Epic\\EpicGamesLauncher\\Data\\Manifests")) else { return };
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let Ok(b) = std::fs::read(e.path()) else { continue };
        let Ok(j) = serde_json::from_slice::<serde_json::Value>(&b) else { continue };
        let s = |k: &str| j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let jeu = j.get("AppCategories").and_then(|c| c.as_array()).is_some_and(|c| c.iter().any(|x| x == "games"));
        if !jeu || j.get("bIsIncompleteInstall").and_then(|x| x.as_bool()) == Some(true) {
            continue;
        }
        let cible = format!("com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true", s("CatalogNamespace"), s("CatalogItemId"), s("AppName"));
        v.push(Item { genre: Genre::Jeu, nom: s("DisplayName"), cible, detail: "Jeu · Epic".into() });
    }
}

/// Projets de dev : sous-dossiers de `C:\Dev` (sans les dossiers cachés).
pub fn projets() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(racine_dev()) else { return vec![] };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
    v.sort();
    v
}

/// URL GitHub du dépôt (remote `origin` de `.git/config`).
pub fn github(projet: &Path) -> Option<String> {
    let t = std::fs::read_to_string(projet.join(".git\\config")).ok()?;
    let mut dans_origin = false;
    for l in t.lines().map(str::trim) {
        if l.starts_with('[') {
            dans_origin = l == "[remote \"origin\"]";
        } else if dans_origin && let Some(u) = l.strip_prefix("url = ") {
            let u = u.trim().trim_end_matches(".git");
            let u = u.strip_prefix("git@github.com:").map(|r| format!("https://github.com/{r}")).unwrap_or_else(|| u.to_string());
            return u.starts_with("https://").then_some(u);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Fichiers : index de l'Explorateur s'il est lisible, sinon un parcours léger des dossiers perso.
// ---------------------------------------------------------------------------------------------

/// Noms ignorés partout (système, caches, dépendances) : ils noieraient les vrais fichiers.
const EXCLUS: &[&str] = &["windows", "program files", "program files (x86)", "programdata", "appdata", "$recycle.bin", "system volume information", "node_modules", "target", ".git", ".cargo", ".rustup", "__pycache__", ".vs", ".vscode", "obj", ".cache", ".npm", ".gradle", "msocache", "$windows.~ws", "windowsapps", ".venv", "venv", "site-packages", "steamapps"];

fn exclu(nom: &str) -> bool {
    let l = nom.to_ascii_lowercase();
    l.starts_with('.') && l != ".config" || EXCLUS.contains(&l.as_str())
}

pub struct Fichiers {
    pub index: socle::fuzzy::Index,
    /// Noms bout à bout (une seule allocation) et leur début ; n + 1 valeurs.
    noms: String,
    debuts: Vec<u32>,
    /// Parent dans `noms` (u32::MAX = racine : le nom est alors un chemin complet).
    parent: Vec<u32>,
    pub dossier: Vec<bool>,
}

impl Fichiers {
    pub fn len(&self) -> usize {
        self.parent.len()
    }
    pub fn nom(&self, i: usize) -> &str {
        &self.noms[self.debuts[i] as usize..self.debuts[i + 1] as usize]
    }
    pub fn chemin(&self, i: usize) -> PathBuf {
        let mut parts = vec![];
        let mut k = i as u32;
        let mut garde = 0;
        while k != u32::MAX && garde < 256 {
            parts.push(self.nom(k as usize));
            k = self.parent[k as usize];
            garde += 1;
        }
        let mut p = PathBuf::new();
        for s in parts.iter().rev() {
            if p.as_os_str().is_empty() && s.len() == 2 && s.ends_with(':') {
                p.push(format!("{s}\\"));
            } else {
                p.push(s);
            }
        }
        p
    }
    fn push(&mut self, nom: &str, parent: u32, dir: bool) -> u32 {
        // Fichiers : léger malus, les dossiers passent devant à score égal.
        self.index.push(nom, if dir { 20 } else { 0 });
        self.noms.push_str(nom);
        self.debuts.push(self.noms.len() as u32);
        self.parent.push(parent);
        self.dossier.push(dir);
        (self.parent.len() - 1) as u32
    }
    fn vide() -> Self {
        Fichiers { index: socle::fuzzy::Index::new(), noms: String::new(), debuts: vec![0], parent: vec![], dossier: vec![] }
    }
}

pub fn index_explorateur() -> Option<PathBuf> {
    env("LOCALAPPDATA").map(|d| d.join("Explorateur\\index.bin"))
}

/// Charge l'index de l'Explorateur (format `EXPLIDX2`/`EXPLIDX1` : parent u32, dossier u8, [mtime u32], longueur u16, nom).
pub fn lire_explorateur(f: &Path) -> Option<Fichiers> {
    let b = std::fs::read(f).ok()?;
    let v2 = match b.get(..8)? {
        b"EXPLIDX2" => true,
        b"EXPLIDX1" => false,
        _ => return None,
    };
    let n = u32::from_le_bytes(b.get(8..12)?.try_into().ok()?) as usize;
    let mut out = Fichiers::vide();
    // Correspondance nœud de l'Explorateur → entrée gardée (u32::MAX = exclu).
    let mut map: Vec<u32> = Vec::with_capacity(n);
    let mut k = 12;
    for _ in 0..n {
        let parent = u32::from_le_bytes(b.get(k..k + 4)?.try_into().ok()?);
        let dir = *b.get(k + 4)? != 0;
        k += if v2 { 9 } else { 5 };
        let len = u16::from_le_bytes(b.get(k..k + 2)?.try_into().ok()?) as usize;
        let nom = std::str::from_utf8(b.get(k + 2..k + 2 + len)?).ok()?;
        k += 2 + len;
        let p = if parent == u32::MAX {
            u32::MAX
        } else {
            match map.get(parent as usize) {
                Some(&m) if m != u32::MAX => m,
                Some(_) => {
                    map.push(u32::MAX);
                    continue;
                }
                None => return None, // fichier abîmé
            }
        };
        if parent != u32::MAX && exclu(nom) {
            map.push(u32::MAX);
            continue;
        }
        map.push(out.push(nom, p, dir || parent == u32::MAX));
    }
    Some(out)
}

/// Repli sans Explorateur : dossiers perso et `C:\Dev`, 8 niveaux, 300 000 entrées au plus.
pub fn parcourir() -> Fichiers {
    let mut out = Fichiers::vide();
    let mut racines: Vec<PathBuf> = vec![];
    if let Some(u) = env("USERPROFILE") {
        for d in ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music", "OneDrive"] {
            racines.push(u.join(d));
        }
    }
    racines.push(racine_dev());
    for r in racines.into_iter().filter(|r| r.is_dir()) {
        let id = out.push(&r.to_string_lossy(), u32::MAX, true);
        marcher(&r, id, 0, &mut out);
    }
    out
}

fn marcher(d: &Path, id: u32, depth: u32, out: &mut Fichiers) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        if out.len() >= 300_000 {
            return;
        }
        let nom = e.file_name().to_string_lossy().into_owned();
        if exclu(&nom) {
            continue;
        }
        let dir = e.file_type().is_ok_and(|t| t.is_dir());
        let k = out.push(&nom, id, dir);
        if dir && depth < 8 {
            marcher(&e.path(), k, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lanceur-test-{nom}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn index_explorateur_relu() {
        // Construit un petit index au format EXPLIDX2 : C: > Users > a.txt, C: > node_modules > x.js
        let mut b = b"EXPLIDX2".to_vec();
        let nodes: &[(u32, bool, &str)] = &[(u32::MAX, true, "C:"), (0, true, "Users"), (1, false, "a.txt"), (0, true, "node_modules"), (3, false, "x.js")];
        b.extend((nodes.len() as u32).to_le_bytes());
        for (p, d, n) in nodes {
            b.extend(p.to_le_bytes());
            b.push(*d as u8);
            b.extend(0u32.to_le_bytes());
            b.extend((n.len() as u16).to_le_bytes());
            b.extend(n.as_bytes());
        }
        let d = tmp("idx");
        let f = d.join("index.bin");
        std::fs::write(&f, b).unwrap();
        let fx = lire_explorateur(&f).unwrap();
        assert_eq!(fx.len(), 3, "node_modules et son contenu sont exclus");
        let hit = fx.index.search("a.txt", 1)[0].index;
        assert_eq!(fx.chemin(hit), PathBuf::from("C:\\Users\\a.txt"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn github_depuis_config() {
        let d = tmp("gh");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git\\config"), "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/Desarrh/lanceur.git\n").unwrap();
        assert_eq!(github(&d).as_deref(), Some("https://github.com/Desarrh/lanceur"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn vdf_simple() {
        let t = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n}";
        assert_eq!(vdf(t, "name").next().as_deref(), Some("Dota 2"));
    }
}
