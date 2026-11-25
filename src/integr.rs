//! Les apps autour : détection (règle 4 : absente ou trop ancienne = entrée masquée), travaux lents
//! demandés par le moteur (`Req`) et exécution des actions. Tout tourne sur des threads de fond.
use crate::moteur::{Action, Apps, Ico, Req, Res, Sys};
use crate::sysw;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from)
}

fn existe(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

/// Valeur d'une chaîne de la ressource de version (`VigieCli`, `MemoireCli`…), lue dans les octets
/// de l'exe : clé UTF-16 + NUL, bourrage, puis la valeur UTF-16 jusqu'au NUL.
pub fn marqueur(exe: &Path, cle: &str) -> Option<String> {
    let b = std::fs::read(exe).ok()?;
    let k: Vec<u8> = cle.encode_utf16().chain(Some(0)).flat_map(|u| u.to_le_bytes()).collect();
    let pos = memfind(&b, &k)? + k.len();
    let mut i = pos;
    while b.get(i..i + 2) == Some(&[0, 0]) {
        i += 2;
    }
    let mut u = vec![];
    while let Some(c) = b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]])) {
        if c == 0 || u.len() > 512 {
            break;
        }
        u.push(c);
        i += 2;
    }
    Some(String::from_utf16_lossy(&u))
}

fn memfind(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn sur_path(nom: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(nom)).find(|p| p.is_file())
}

/// Le `claude.exe` le plus récent (PATH, sinon dossier versionné de l'app Claude).
fn claude() -> Option<PathBuf> {
    if let Some(p) = sur_path("claude.exe") {
        return Some(p);
    }
    let base = env("APPDATA")?.join("Claude\\claude-code");
    // claude-code\<version>\[<empreinte>\]claude.exe
    let mut v: Vec<PathBuf> = vec![];
    for e in std::fs::read_dir(base).ok()?.flatten() {
        v.push(e.path().join("claude.exe"));
        v.extend(std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|f| f.path().join("claude.exe")));
    }
    v.retain(|p| p.is_file());
    v.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    v.pop()
}

pub fn detecter() -> Apps {
    let dev = crate::sources::racine_dev();
    let tube = existe(dev.join("tube\\dist\\Tube.exe"));
    let vigie = existe(dev.join("vigie\\dist\\Vigie.exe"));
    let memoire = existe(dev.join("memoire\\dist\\Memoire.exe"));
    let bilan = existe(dev.join("bilan\\dist\\Bilan.exe"));
    let taches = existe(dev.join("taches\\dist\\Taches.exe"));
    let a_mot = |exe: &Option<PathBuf>, cle: &str, mot: &str| exe.as_deref().and_then(|e| marqueur(e, cle)).is_some_and(|m| m.split_whitespace().any(|x| x == mot));
    let local = env("LOCALAPPDATA").unwrap_or_default();
    Apps {
        vigie_dev: a_mot(&vigie, "VigieCli", "etat-dev") && a_mot(&vigie, "VigieCli", "couper-dev"),
        memoire_cherche: a_mot(&memoire, "MemoireCli", "chercher"),
        bilan_resume: a_mot(&bilan, "BilanCli", "resume"),
        taches_cli: ["ajouter", "comprendre", "lister", "cocher"].iter().all(|m| a_mot(&taches, "TachesCli", m)),
        taches,
        tube,
        vigie,
        memoire,
        bilan,
        code: existe(local.join("Programs\\Microsoft VS Code\\Code.exe"))
            .or_else(|| env("ProgramFiles").and_then(|p| existe(p.join("Microsoft VS Code\\Code.exe"))))
            .or_else(|| sur_path("code.cmd")),
        wt: existe(local.join("Microsoft\\WindowsApps\\wt.exe")),
        claude: claude(),
    }
}

// ---------------------------------------------------------------------------------------------
// Travaux lents
// ---------------------------------------------------------------------------------------------

fn ligne(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string()
}

fn mo(v: f64) -> String {
    if v >= 1024.0 { format!("{:.1} Go", v / 1024.0).replace('.', ",") } else { format!("{v:.0} Mo") }
}

pub fn faire(r: &Req, apps: &Apps) -> Vec<Res> {
    match r {
        Req::Fichiers(_) => vec![], // fait par App avec l'index des fichiers
        Req::Memoire(q) => memoire(q, apps),
        Req::VigieDev => vigie_dev(apps).unwrap_or_default(),
        Req::Ram => ram(apps),
        Req::Bilan => bilan(apps),
        Req::Claude => sessions_claude(apps),
        Req::Sorties => sorties(),
        Req::TachesComprendre(t) => taches_comprendre(t, apps),
        Req::TachesLister(q) => taches_lister(q.as_deref(), apps),
    }
}

fn memoire(q: &str, apps: &Apps) -> Vec<Res> {
    let Some(exe) = &apps.memoire else { return vec![] };
    let Ok((0, out)) = sysw::run_capture(exe, &["--chercher", q, "--json", "--max", "20"], Duration::from_millis(1500)) else {
        return vec![Res::new(Ico::Memoire, "Mémoire ne répond pas", "", Action::Rien)];
    };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(ligne(&out).as_str()) else { return vec![] };
    let mut v: Vec<Res> = j
        .get("resultats")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let id = x.get("id")?.as_str()?.to_string();
                    let apercu = x.get("apercu").and_then(|s| s.as_str()).unwrap_or("").to_string();
                    let image = x.get("type").and_then(|s| s.as_str()) == Some("image");
                    let quand = x.get("quand").and_then(|s| s.as_u64()).unwrap_or(0);
                    let age = crate::moteur::maintenant().saturating_sub(quand) as f32;
                    let sous = format!("{} · {} · Entrée recopie", if image { "Capture" } else { "Copie" }, socle::ui::ago(age));
                    Some(Res::new(Ico::Memoire, apercu, sous, Action::Sortie { exe: exe.to_string_lossy().into_owned(), args: vec!["--copier".into(), id] }))
                })
                .collect()
        })
        .unwrap_or_default();
    if v.is_empty() {
        v.push(Res::new(Ico::Memoire, format!("Mémoire : rien pour « {q} »"), "", Action::Rien));
    }
    v
}

fn vigie_dev(apps: &Apps) -> Option<Vec<Res>> {
    let exe = apps.vigie.as_ref()?;
    let (_, out) = sysw::run_capture(exe, &["--etat-dev"], Duration::from_secs(3)).ok()?;
    let j: serde_json::Value = serde_json::from_str(&ligne(&out)).ok()?;
    let e = exe.to_string_lossy().into_owned();
    let items = j.get("items")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|it| {
                let s = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let cle = s("cle");
                if cle.is_empty() {
                    return None;
                }
                let ports: Vec<String> = it.get("ports").and_then(|p| p.as_array()).map(|a| a.iter().filter_map(|p| p.as_u64()).map(|p| format!(":{p}")).collect()).unwrap_or_default();
                let ram = it.get("ram_mo").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let mut sous = format!("{} · {}", s("kind"), mo(ram));
                if !ports.is_empty() {
                    sous += &format!(" · {}", ports.join(" "));
                }
                if !s("projet").is_empty() {
                    sous += &format!(" · {}", s("projet"));
                }
                let titre = format!("Couper {}", s("label"));
                Some(Res::new(Ico::Tuer, titre, sous, Action::Sortie { exe: e.clone(), args: vec!["--couper-dev".into(), "--seulement".into(), cle] }).confirmer())
            })
            .collect(),
    )
}

fn ram(apps: &Apps) -> Vec<Res> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut v = vec![];
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        let go = |b: u64| format!("{:.1}", b as f64 / 1_073_741_824.0).replace('.', ",");
        let used = m.ullTotalPhys - m.ullAvailPhys;
        let titre = format!("RAM : {} / {} Go ({} %)", go(used), go(m.ullTotalPhys), m.dwMemoryLoad);
        v.push(Res::new(Ico::Jauge, titre.clone(), format!("{} Go libres · Entrée copie", go(m.ullAvailPhys)), Action::Copier(titre)));
    }
    if apps.vigie_dev && let Some(dev) = vigie_dev(apps) {
        if !dev.is_empty() {
            v.push(Res::new(Ico::Tuer, format!("{} serveur(s) de dev ouverts", dev.len()), "Entrée : les voir (k)", Action::Remplir("k ".into())));
        }
    }
    if let Some(vg) = &apps.vigie {
        v.push(Res::new(Ico::Jauge, "Ouvrir Vigie", "Gestionnaire de tâches", Action::Ouvrir { cible: vg.to_string_lossy().into_owned(), args: None, dossier: None }));
    }
    v
}

fn bilan(apps: &Apps) -> Vec<Res> {
    let Some(exe) = &apps.bilan else { return vec![] };
    let l = match sysw::run_capture(exe, &["--resume"], Duration::from_millis(1500)) {
        Ok((0, out)) => ligne(&out),
        _ => String::new(),
    };
    let mut v = vec![];
    if !l.is_empty() {
        v.push(Res::new(Ico::Jauge, l.clone(), "Bilan · Entrée copie", Action::Copier(l)));
    }
    v.push(Res::new(Ico::Jauge, "Ouvrir Bilan", "La semaine en détail", Action::Ouvrir { cible: exe.to_string_lossy().into_owned(), args: None, dossier: None }));
    v
}

/// « liste · échéance · ★ · ×n » d'une tâche ou d'un aperçu JSON de Tâches.
fn infos_tache(j: &serde_json::Value) -> String {
    let mut v: Vec<String> = vec![];
    if let Some(l) = j.get("liste").and_then(|x| x.as_str()).filter(|l| !l.is_empty()) {
        v.push(l.to_string());
    }
    if let Some(e) = j.get("echeance").and_then(|x| x.as_str()) {
        v.push(e.to_string());
    }
    if j.get("important").and_then(|x| x.as_bool()) == Some(true) {
        v.push("★ important".into());
    }
    if let Some(n) = j.get("compteur").and_then(|x| x.as_u64()) {
        v.push(format!("×{n}"));
    }
    v.join(" · ")
}

fn taches_comprendre(t: &str, apps: &Apps) -> Vec<Res> {
    let Some(exe) = &apps.taches else { return vec![] };
    let Ok((0, out)) = sysw::run_capture(exe, &["--comprendre", t, "--json"], Duration::from_millis(800)) else { return vec![] };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&ligne(&out)) else { return vec![] };
    let titre = j.get("titre").and_then(|x| x.as_str()).unwrap_or(t).to_string();
    let infos = infos_tache(&j);
    vec![Res::new(Ico::Tache, titre, if infos.is_empty() { "Ma journée".into() } else { infos }, Action::Rien)]
}

fn taches_lister(q: Option<&str>, apps: &Apps) -> Vec<Res> {
    let Some(exe) = &apps.taches else { return vec![] };
    let mut a = vec!["--lister", if q.is_some() { "tout" } else { "ma-journee" }, "--json", "--max", "20"];
    if let Some(q) = q {
        a.extend(["--chercher", q]);
    }
    let Ok((0, out)) = sysw::run_capture(exe, &a, Duration::from_millis(1500)) else { return vec![] };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&ligne(&out)) else { return vec![] };
    let e = exe.to_string_lossy().into_owned();
    j.get("taches")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    let id = match t.get("id")? {
                        serde_json::Value::String(s) => s.clone(),
                        x => x.to_string(),
                    };
                    let titre = t.get("titre")?.as_str()?.to_string();
                    let infos = infos_tache(t);
                    let mut r = Res::new(Ico::Tache, titre, if infos.is_empty() { "Entrée coche".to_string() } else { format!("{infos} · Entrée coche") }, Action::Sortie { exe: e.clone(), args: vec!["--cocher".into(), id] });
                    r.alt = Some(Action::Ouvrir { cible: e.clone(), args: None, dossier: None });
                    r.section = Some(if q.is_some() { "Tâches trouvées" } else { "Ma journée" }.into());
                    Some(r)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn sorties() -> Vec<Res> {
    match crate::audio::outputs() {
        Ok(d) => d
            .into_iter()
            .map(|d| {
                let sous = if d.default { "Sortie actuelle" } else { "Entrée : en faire la sortie par défaut" };
                Res::new(Ico::Son, d.name, sous, if d.default { Action::Rien } else { Action::Sys(Sys::Sortie(d.id)) })
            })
            .collect(),
        Err(e) => vec![Res::new(Ico::Son, "Sorties audio illisibles", e, Action::Rien)],
    }
}

/// Sessions Claude Code récentes (`~\.claude\projects\<dossier>\<id>.jsonl`), reprises dans un terminal.
fn sessions_claude(apps: &Apps) -> Vec<Res> {
    let Some(claude) = &apps.claude else { return vec![] };
    let Some(base) = env("USERPROFILE").map(|u| u.join(".claude\\projects")) else { return vec![] };
    let mut f: Vec<(std::time::SystemTime, PathBuf)> = vec![];
    for d in std::fs::read_dir(base).into_iter().flatten().flatten() {
        for e in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "jsonl") && let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                f.push((t, p));
            }
        }
    }
    f.sort_by(|a, b| b.0.cmp(&a.0));
    f.into_iter()
        .take(40)
        .filter_map(|(t, p)| {
            let id = p.file_stem()?.to_string_lossy().into_owned();
            let (cwd, premier) = entete_session(&p);
            let cwd = cwd?;
            let age = t.elapsed().map_or(0.0, |d| d.as_secs_f32());
            let projet = Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(cwd.clone());
            let titre = match premier {
                Some(m) => format!("{projet} · {m}"),
                None => projet,
            };
            let sous = format!("{} · {cwd}", socle::ui::ago(age));
            let cl = claude.to_string_lossy();
            let (cible, args) = match &apps.wt {
                Some(wt) => (wt.to_string_lossy().into_owned(), format!("-d \"{cwd}\" \"{cl}\" --resume {id}")),
                None => ("cmd.exe".to_string(), format!("/k \"\"{cl}\" --resume {id}\"")),
            };
            Some(Res::new(Ico::Claude, titre, sous, Action::Ouvrir { cible, args: Some(args), dossier: Some(cwd) }))
        })
        .take(25)
        .collect()
}

/// Dossier de travail et début du premier message, lus dans les 64 premières lignes.
fn entete_session(p: &Path) -> (Option<String>, Option<String>) {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(p) else { return (None, None) };
    let (mut cwd, mut premier) = (None, None);
    for l in std::io::BufReader::new(f).lines().take(64).map_while(Result::ok) {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&l) else { continue };
        if cwd.is_none() {
            cwd = j.get("cwd").and_then(|c| c.as_str()).map(String::from);
        }
        if premier.is_none() && j.get("type").and_then(|t| t.as_str()) == Some("user") {
            let c = j.pointer("/message/content");
            let txt = c.and_then(|c| c.as_str()).map(String::from).or_else(|| c.and_then(|c| c.as_array()).and_then(|a| a.iter().find_map(|x| x.get("text").and_then(|t| t.as_str()).map(String::from))));
            if let Some(t) = txt.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.starts_with('<')) {
                premier = Some(t.chars().take(70).collect::<String>().replace(['\n', '\r'], " "));
            }
        }
        if cwd.is_some() && premier.is_some() {
            break;
        }
    }
    (cwd, premier)
}

// ---------------------------------------------------------------------------------------------
// Exécution
// ---------------------------------------------------------------------------------------------

/// Exécute une action (thread de fond). Renvoie un message à afficher, le cas échéant.
pub fn executer(a: &Action) -> Result<Option<String>, String> {
    match a {
        Action::Ouvrir { cible, args, dossier } => sysw::open(cible, args.as_deref(), dossier.as_deref().map(Path::new)).map(|_| None),
        Action::Reveler(p) => sysw::reveal(Path::new(p)).map(|_| None),
        Action::Copier(t) => sysw::set_clipboard(t).map(|_| Some("Copié dans le presse-papiers".into())),
        Action::Sortie { exe, args } => {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            let (code, out) = sysw::run_capture(Path::new(exe), &a, Duration::from_secs(15))?;
            let l = ligne(&out);
            if code != 0 && args.first().map(String::as_str) == Some("--copier") {
                return Err("Mémoire n'a pas copié cet élément (protégé ou disparu)".into());
            }
            if code != 0 && l.is_empty() {
                return Err(format!("échec (code {code})"));
            }
            match args.first().map(String::as_str) {
                Some("--ajouter") => return Ok(Some("Tâche ajoutée".into())),
                Some("--cocher") => return Ok(Some("Tâche cochée".into())),
                _ => {}
            }
            Ok(Some(if l.is_empty() { "Fait".into() } else { l }))
        }
        Action::Tuer { pid, exe } => sysw::kill(*pid, Path::new(exe)).map(|_| Some(format!("Processus {pid} arrêté"))),
        Action::Sys(s) => match s {
            Sys::Verrouiller => sysw::lock().map(|_| None),
            Sys::Veille => sysw::sleep().map(|_| None),
            Sys::ViderCorbeille => sysw::empty_recycle_bin().map(|_| None),
            Sys::Volume(v) => crate::audio::set_volume(*v).map(|_| Some(format!("Volume à {:.0} %", v * 100.0))),
            Sys::Muet => crate::audio::toggle_mute().map(|m| Some(if m { "Son coupé".into() } else { "Son remis".into() })),
            Sys::Sortie(id) => crate::audio::set_default_output(id).map(|_| Some("Sortie audio changée".into())),
            Sys::Recharger | Sys::Reglages | Sys::Quitter => Ok(None), // faits par l'app
        },
        Action::Remplir(_) | Action::Rien => Ok(None),
    }
}

/// Taux de change BCE : lus dans le cache, rafraîchis une fois par jour par curl.exe (fourni avec Windows).
pub fn taux(rafraichir: bool) -> crate::calc::Rates {
    let f = socle::paths::cache_dir("Lanceur").join("taux-bce.xml");
    let vieux = std::fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_none_or(|d| d > Duration::from_secs(86_400));
    if rafraichir && vieux {
        let curl = env("SystemRoot").unwrap_or_else(|| PathBuf::from("C:\\Windows")).join("System32\\curl.exe");
        if let Ok((0, xml)) = sysw::run_capture(&curl, &["-sSfL", "--max-time", "8", "--proto", "=https", "--", "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml"], Duration::from_secs(10)) {
            if crate::calc::parse_ecb(&xml).is_some() {
                let _ = std::fs::create_dir_all(f.parent().unwrap_or(Path::new(".")));
                let _ = socle::paths::write_atomic(&f, xml.as_bytes());
            }
        }
    }
    std::fs::read_to_string(&f).ok().and_then(|x| crate::calc::parse_ecb(&x)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marqueur_lu_dans_un_faux_exe() {
        let d = std::env::temp_dir().join(format!("lanceur-test-marq-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let f = d.join("Faux.exe");
        let mut b = b"MZ....".to_vec();
        let u = |s: &str| s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(u("VigieCli\0"));
        b.extend([0, 0]);
        b.extend(u("avant-jeu etat-dev couper-dev\0"));
        std::fs::write(&f, b).unwrap();
        assert_eq!(marqueur(&f, "VigieCli").as_deref(), Some("avant-jeu etat-dev couper-dev"));
        assert_eq!(marqueur(&f, "MemoireCli"), None);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn derniere_ligne() {
        assert_eq!(ligne("a\n\n b \n\n"), "b");
    }
}
