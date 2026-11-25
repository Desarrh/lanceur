//! Le moteur : une requête tapée → une liste de résultats, chacun avec son action.
//! Pur et synchrone (testable) : ce qui est lent (fichiers, Vigie, Mémoire…) est demandé par une
//! `Req`, calculé ailleurs sur un thread, puis relu dans `Ctx::asyncs` au tour suivant.
use crate::calc;
use crate::sources::{Genre, Item};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ico {
    App,
    Maison,
    Jeu,
    Dossier,
    Fichier,
    Calc,
    Web,
    Commande,
    Code,
    Terminal,
    Git,
    Copier,
    Tuer,
    Son,
    Memoire,
    Tache,
    Jauge,
    Video,
    Repeter,
    Claude,
    Texte,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Sys {
    Verrouiller,
    Veille,
    ViderCorbeille,
    Volume(f32),
    Muet,
    Sortie(String),
    Recharger,
    Reglages,
    Quitter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// ShellExecute : fichier, dossier, .lnk, URL, exe.
    Ouvrir { cible: String, args: Option<String>, dossier: Option<String> },
    /// Ouvrir le dossier parent, fichier sélectionné.
    Reveler(String),
    Copier(String),
    /// Lance un exe et affiche sa sortie (une ligne) dans la barre, qui reste ouverte.
    Sortie { exe: String, args: Vec<String> },
    Tuer { pid: u32, exe: String },
    Sys(Sys),
    /// Remplace le texte de la barre (complétion).
    Remplir(String),
    Rien,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Res {
    pub ico: Ico,
    pub titre: String,
    pub sous: String,
    pub action: Action,
    /// Maj+Entrée.
    #[serde(default)]
    pub alt: Option<Action>,
    /// Clé d'apprentissage (fréquence et récence).
    #[serde(default)]
    pub cle: Option<String>,
    /// Demande une 2e validation (Entrée deux fois).
    #[serde(default)]
    pub confirmer: bool,
    #[serde(default)]
    pub couleur: Option<[u8; 3]>,
    /// Image : `p:<chemin>` icône Windows du fichier, `t:<ext>` icône d'un type, `j:<clé>|<appid>|<url>` jaquette.
    #[serde(default)]
    pub img: Option<String>,
    /// Section imposée (sinon déduite de l'icône).
    #[serde(default)]
    pub section: Option<String>,
}

impl Res {
    pub fn new(ico: Ico, titre: impl Into<String>, sous: impl Into<String>, action: Action) -> Res {
        Res { ico, titre: titre.into(), sous: sous.into(), action, alt: None, cle: None, confirmer: false, couleur: None, img: None, section: None }
    }
    pub fn cle(mut self, c: impl Into<String>) -> Res {
        self.cle = Some(c.into());
        self
    }
    pub fn confirmer(mut self) -> Res {
        self.confirmer = true;
        self
    }
}

/// Travail lent demandé par le moteur.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Req {
    Fichiers(String),
    Memoire(String),
    VigieDev,
    Ram,
    Bilan,
    Claude,
    Sorties,
    /// `Taches.exe --comprendre "<texte>" --json` : aperçu de la tâche qui sera ajoutée.
    TachesComprendre(String),
    /// `Taches.exe --lister … --json` : `None` = Ma journée, `Some(t)` = recherche dans tout.
    TachesLister(Option<String>),
}

/// Apps maison trouvées, avec ce que leur CLI sait faire (règle 4 : absente = entrée masquée).
#[derive(Clone, Debug, Default)]
pub struct Apps {
    pub tube: Option<PathBuf>,
    pub vigie: Option<PathBuf>,
    /// Vigie déclare `etat-dev couper-dev` dans `VigieCli`.
    pub vigie_dev: bool,
    pub memoire: Option<PathBuf>,
    pub memoire_cherche: bool,
    pub bilan: Option<PathBuf>,
    pub bilan_resume: bool,
    pub taches: Option<PathBuf>,
    /// `TachesCli` contient `ajouter comprendre lister cocher`.
    pub taches_cli: bool,
    pub code: Option<PathBuf>,
    pub wt: Option<PathBuf>,
    pub claude: Option<PathBuf>,
}

/// Mémoire des choix : fréquence + récence par clé, et dernier choix pour une requête exacte.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Appris {
    #[serde(default)]
    pub usage: HashMap<String, (u32, u64)>,
    #[serde(default)]
    pub requetes: HashMap<String, String>,
}

pub fn maintenant() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Appris {
    /// Bonus de score (0–300) : fréquence plafonnée + récence.
    pub fn bonus(&self, cle: &str) -> i32 {
        let Some(&(n, t)) = self.usage.get(cle) else { return 0 };
        let age = maintenant().saturating_sub(t);
        let recent = if age < 3600 { 120 } else if age < 86_400 { 80 } else if age < 7 * 86_400 { 40 } else if age < 30 * 86_400 { 15 } else { 0 };
        (n.min(30) as i32) * 6 + recent
    }
    pub fn noter(&mut self, requete: &str, cle: &str) {
        let e = self.usage.entry(cle.to_string()).or_insert((0, 0));
        e.0 = e.0.saturating_add(1);
        e.1 = maintenant();
        let q = socle::fuzzy::fold(requete.trim());
        if !q.is_empty() && q.len() <= 24 {
            self.requetes.insert(q, cle.to_string());
        }
        // Borne : on oublie les moins utiles.
        if self.usage.len() > 2000 {
            let mut v: Vec<_> = self.usage.iter().map(|(k, (n, t))| (k.clone(), *n as u64 * 86_400 + t)).collect();
            v.sort_by_key(|x| x.1);
            for (k, _) in v.iter().take(500) {
                self.usage.remove(k);
            }
        }
        if self.requetes.len() > 1000 {
            self.requetes.clear();
        }
    }
}

pub struct Ctx<'a> {
    pub items: &'a [Item],
    pub index: &'a socle::fuzzy::Index,
    pub appris: &'a Appris,
    pub taux: &'a calc::Rates,
    pub snippets: &'a BTreeMap<String, String>,
    pub projets: &'a [PathBuf],
    pub apps: &'a Apps,
    pub sel: Option<&'a str>,
    pub derniere: Option<&'a Res>,
    pub asyncs: &'a HashMap<Req, Vec<Res>>,
}

/// Commandes intégrées : (nom cherché, sous-titre, icône, action, confirmer).
fn commandes(apps: &Apps) -> Vec<Res> {
    let mut v = vec![
        Res::new(Ico::Commande, "Verrouiller", "Windows · verrouiller la session", Action::Sys(Sys::Verrouiller)),
        Res::new(Ico::Commande, "Veille", "Windows · mettre en veille", Action::Sys(Sys::Veille)).confirmer(),
        Res::new(Ico::Commande, "Vider la corbeille", "Windows · Windows demandera confirmation", Action::Sys(Sys::ViderCorbeille)),
        Res::new(Ico::Dossier, "Corbeille", "Windows · ouvrir", Action::Ouvrir { cible: "shell:RecycleBinFolder".into(), args: None, dossier: None }),
        Res::new(Ico::Son, "Muet", "Son · couper ou remettre", Action::Sys(Sys::Muet)),
        Res::new(Ico::Son, "Volume", "Son · tape « vol 30 »", Action::Remplir("vol ".into())),
        Res::new(Ico::Son, "Sortie audio", "Son · changer de périphérique", Action::Remplir("sortie ".into())),
        Res::new(Ico::Commande, "Paramètres du Lanceur", "Ouvre settings.json (snippets, réglages)", Action::Sys(Sys::Reglages)),
        Res::new(Ico::Commande, "Recharger l'index du Lanceur", "Relit apps, jeux et fichiers", Action::Sys(Sys::Recharger)),
        Res::new(Ico::Commande, "Quitter le Lanceur", "Ferme l'app (Alt+Espace ne marchera plus)", Action::Sys(Sys::Quitter)),
    ];
    if let Some(vg) = &apps.vigie {
        let e = vg.to_string_lossy().into_owned();
        v.push(Res::new(Ico::Jeu, "Mode jeu", "Vigie · endort les apps lourdes, plan performances", Action::Sortie { exe: e.clone(), args: vec!["--avant-jeu".into()] }).confirmer());
        v.push(Res::new(Ico::Jeu, "Fin du mode jeu", "Vigie · réveille les apps, rétablit le plan", Action::Sortie { exe: e, args: vec!["--apres-jeu".into()] }));
        v.push(Res::new(Ico::Jauge, "RAM", "Mémoire utilisée", Action::Remplir("ram".into())));
    }
    if apps.bilan_resume {
        v.push(Res::new(Ico::Jauge, "Bilan", "Résumé de la semaine", Action::Remplir("bilan".into())));
    }
    v
}

/// Remplace les lettres d'une frappe AZERTY ↔ QWERTY (a/q, z/w, m/;).
pub fn clavier_inverse(q: &str) -> String {
    q.chars()
        .map(|c| match c {
            'a' => 'q',
            'q' => 'a',
            'z' => 'w',
            'w' => 'z',
            'A' => 'Q',
            'Q' => 'A',
            'Z' => 'W',
            'W' => 'Z',
            ';' => 'm',
            c => c,
        })
        .collect()
}

fn url(s: &str) -> String {
    let mut o = String::new();
    for b in s.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn web(titre: &str, sous: &str, u: String) -> Res {
    Res::new(Ico::Web, titre, sous, Action::Ouvrir { cible: u, args: None, dossier: None })
}

fn ico(g: Genre) -> Ico {
    match g {
        Genre::App => Ico::App,
        Genre::Maison => Ico::Maison,
        Genre::Jeu => Ico::Jeu,
        Genre::Commande => Ico::Commande,
    }
}

fn item_res(it: &Item) -> Res {
    let action = Action::Ouvrir { cible: it.cible.clone(), args: None, dossier: None };
    let mut r = Res::new(ico(it.genre), it.nom.clone(), it.detail.clone(), action).cle(it.cible.clone());
    r.img = it.img.clone();
    if !it.cible.contains("://") {
        r.alt = Some(Action::Reveler(it.cible.clone()));
    }
    r
}

/// Recherche floue dans les apps et jeux (+ commandes), avec repli AZERTY ↔ QWERTY.
fn chercher(q: &str, cx: &Ctx, seulement: Option<Genre>, limit: usize) -> Vec<(i32, Res)> {
    let mut out: Vec<(i32, Res)> = vec![];
    let mut vus = std::collections::HashSet::new();
    let mut une = |q: &str, malus: i32, out: &mut Vec<(i32, Res)>| {
        for h in cx.index.search(q, limit * 2) {
            let it = &cx.items[h.index];
            if seulement.is_some_and(|g| g != it.genre) || !vus.insert(h.index) {
                continue;
            }
            out.push((h.score + cx.appris.bonus(&it.cible) - malus, item_res(it)));
        }
    };
    une(q, 0, &mut out);
    let inv = clavier_inverse(q);
    if inv != q && out.iter().all(|(s, _)| *s < 1500) {
        une(&inv, 400, &mut out);
    }
    if seulement.is_none() {
        for c in commandes(cx.apps) {
            let s = socle::fuzzy::score(&c.titre, q).or_else(|| socle::fuzzy::score(&c.titre, &inv).map(|s| s - 400));
            if let Some(s) = s {
                let cle = format!("cmd:{}", c.titre);
                out.push((s + cx.appris.bonus(&cle) + 50, c.cle(cle)));
            }
        }
    }
    out
}

fn trier(mut v: Vec<(i32, Res)>, q: &str, cx: &Ctx) -> Vec<Res> {
    // Le dernier choix pour exactement cette requête passe devant.
    if let Some(cle) = cx.appris.requetes.get(&socle::fuzzy::fold(q.trim())) {
        for (s, r) in v.iter_mut() {
            if r.cle.as_deref() == Some(cle) {
                *s += 10_000;
            }
        }
    }
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|x| x.1).collect()
}

/// Actions sur le texte sélectionné dans l'app au premier plan.
fn selection(sel: &str, cx: &Ctx) -> Vec<Res> {
    let court: String = sel.chars().take(40).collect::<String>().replace(['\n', '\r'], " ");
    let court = if sel.chars().count() > 40 { format!("{court}…") } else { court };
    let mut v = vec![];
    let t = sel.trim();
    if (t.starts_with("http://") || t.starts_with("https://")) && !t.contains(char::is_whitespace) {
        v.push(web("Ouvrir le lien", &court, t.to_string()));
    }
    if (t.len() > 2 && t.as_bytes().get(1) == Some(&b':') || t.starts_with("\\\\")) && std::path::Path::new(t).exists() {
        v.push(Res::new(Ico::Dossier, "Ouvrir le chemin", &court, Action::Ouvrir { cible: t.to_string(), args: None, dossier: None }));
    }
    if let Some(c) = calc::eval(t, cx.taux) {
        v.push(Res::new(Ico::Calc, c.display.clone(), format!("Sélection « {court} » · Entrée copie"), Action::Copier(c.copy.clone())));
    }
    if (t.starts_with('{') || t.starts_with('[')) && let Ok(j) = serde_json::from_str::<serde_json::Value>(t) {
        let joli = serde_json::to_string_pretty(&j).unwrap_or_default();
        v.push(Res::new(Ico::Code, "JSON : formater et copier", &court, Action::Copier(joli)));
        let compact = serde_json::to_string(&j).unwrap_or_default();
        v.push(Res::new(Ico::Code, "JSON : compacter et copier", &court, Action::Copier(compact)));
    }
    v.push(web("Traduire en français", &court, format!("https://translate.google.com/?sl=auto&tl=fr&text={}", url(t))));
    v.push(web("Traduire en anglais", &court, format!("https://translate.google.com/?sl=auto&tl=en&text={}", url(t))));
    v.push(web("Chercher sur le web", &court, format!("https://www.google.com/search?q={}", url(t))));
    v
}

/// Résultats pour une requête, et le travail lent à lancer.
pub fn requete(q: &str, cx: &Ctx) -> (Vec<Res>, Vec<Req>) {
    let mut reqs = vec![];
    let r = requete_(q, cx, &mut reqs);
    (r, reqs)
}

fn asyncs(cx: &Ctx, r: &Req, reqs: &mut Vec<Req>) -> Vec<Res> {
    match cx.asyncs.get(r) {
        Some(v) => v.clone(),
        None => {
            reqs.push(r.clone());
            vec![]
        }
    }
}

fn requete_(q: &str, cx: &Ctx, reqs: &mut Vec<Req>) -> Vec<Res> {
    let brut = q;
    let q = q.trim_start();
    if q.is_empty() {
        let mut v = cx.sel.map(|s| selection(s, cx)).unwrap_or_default();
        for r in v.iter_mut() {
            r.section = Some("Texte sélectionné".into());
        }
        if let Some(d) = cx.derniere {
            let mut r = d.clone();
            r.sous = format!("! · refaire · {}", r.sous);
            r.confirmer = false;
            r.section = Some("Dernière action".into());
            v.push(r);
        }
        // Récents et fréquents.
        let mut top: Vec<(i32, &Item)> = cx.items.iter().map(|i| (cx.appris.bonus(&i.cible), i)).filter(|x| x.0 > 0).collect();
        top.sort_by(|a, b| b.0.cmp(&a.0));
        v.extend(top.into_iter().take(7).map(|(_, i)| {
            let mut r = item_res(i);
            r.section = Some("Récents et fréquents".into());
            r
        }));
        return v;
    }
    if q == "!" {
        return cx.derniere.cloned().into_iter().collect();
    }
    let (tete, reste) = match q.split_once(' ') {
        Some((a, b)) => (a.to_lowercase(), b.trim()),
        None => (q.to_lowercase(), ""),
    };
    let a_espace = q.contains(' ');

    // Préfixes web.
    if a_espace {
        let web_pref = match tete.as_str() {
            "g" => Some(("Google", "https://www.google.com/search?q=")),
            "yt" => Some(("YouTube", "https://www.youtube.com/results?search_query=")),
            "gh" => Some(("GitHub", "https://github.com/search?q=")),
            _ => None,
        };
        if let Some((nom, base)) = web_pref {
            if reste.is_empty() {
                return vec![Res::new(Ico::Web, format!("{nom} : tape ta recherche"), "", Action::Rien)];
            }
            return vec![web(&format!("{nom} : {reste}"), "Recherche web", format!("{base}{}", url(reste)))];
        }
    }

    // Branché sur les apps maison.
    match tete.as_str() {
        "y" if a_espace => {
            if let Some(t) = &cx.apps.tube {
                if reste.is_empty() {
                    return vec![Res::new(Ico::Video, "Tube : tape ta recherche", "", Action::Rien)];
                }
                return vec![Res::new(Ico::Video, format!("Tube : {reste}"), "Recherche dans Tube", Action::Ouvrir { cible: t.to_string_lossy().into_owned(), args: Some(quote(reste)), dossier: None })];
            }
        }
        "v" if a_espace && cx.apps.memoire_cherche => {
            if reste.chars().count() < 2 {
                return vec![Res::new(Ico::Memoire, "Mémoire : tape ce que tu cherches", "Presse-papiers et captures", Action::Rien)];
            }
            let v = asyncs(cx, &Req::Memoire(reste.to_string()), reqs);
            return if v.is_empty() && !reqs.is_empty() { vec![Res::new(Ico::Memoire, "Mémoire : recherche…", reste, Action::Rien)] } else { v };
        }
        "t" if (a_espace || tete == "t") && cx.apps.taches_cli => return taches(reste, cx, reqs),
        "k" if a_espace => return tuer(reste, cx, reqs),
        "p" if a_espace => {
            return trier(chercher(reste, cx, Some(Genre::Jeu), 8), reste, cx);
        }
        "d" if a_espace || tete == "d" => return dev(reste, cx),
        "c" if (a_espace || tete == "c") && cx.apps.claude.is_some() => {
            let v = asyncs(cx, &Req::Claude, reqs);
            if reste.is_empty() {
                return v.into_iter().take(12).collect();
            }
            let mut s: Vec<(i32, Res)> = v.into_iter().filter_map(|r| socle::fuzzy::score(&format!("{} {}", r.titre, r.sous), reste).map(|s| (s, r))).collect();
            s.sort_by(|a, b| b.0.cmp(&a.0));
            return s.into_iter().map(|x| x.1).take(12).collect();
        }
        "vol" | "volume" if a_espace => {
            if let Ok(n) = reste.trim_end_matches('%').trim().parse::<f32>() {
                let n = n.clamp(0.0, 100.0);
                return vec![Res::new(Ico::Son, format!("Volume à {n} %"), "Son", Action::Sys(Sys::Volume(n / 100.0)))];
            }
        }
        "sortie" | "audio" => {
            let v = asyncs(cx, &Req::Sorties, reqs);
            let mut s: Vec<(i32, Res)> = v.into_iter().filter_map(|r| if reste.is_empty() { Some((0, r)) } else { socle::fuzzy::score(&r.titre, reste).map(|s| (s, r)) }).collect();
            s.sort_by(|a, b| b.0.cmp(&a.0));
            return s.into_iter().map(|x| x.1).collect();
        }
        _ => {}
    }
    if (tete == "ram" && !a_espace) && cx.apps.vigie.is_some() || tete == "ram" && reste.is_empty() {
        return asyncs(cx, &Req::Ram, reqs);
    }
    if tete == "bilan" && reste.is_empty() && cx.apps.bilan_resume {
        let v = asyncs(cx, &Req::Bilan, reqs);
        return if v.is_empty() { vec![Res::new(Ico::Jauge, "Bilan : calcul…", "", Action::Rien)] } else { v };
    }

    // Snippets ;nom
    if let Some(n) = q.strip_prefix(';') {
        let mut v: Vec<(i32, Res)> = cx
            .snippets
            .iter()
            .filter_map(|(k, t)| {
                let s = if n.is_empty() { Some(0) } else { socle::fuzzy::score(k, n) }?;
                let apercu: String = t.chars().take(60).collect::<String>().replace('\n', " ⏎ ");
                Some((s, Res::new(Ico::Copier, format!(";{k}"), apercu, Action::Copier(t.clone())).cle(format!("snip:{k}"))))
            })
            .collect();
        if v.is_empty() {
            return vec![Res::new(Ico::Copier, "Aucun snippet", "Ajoute-les dans « snippets » des paramètres", Action::Sys(Sys::Reglages))];
        }
        v.sort_by(|a, b| b.0.cmp(&a.0));
        return v.into_iter().map(|x| x.1).collect();
    }

    // :3000 → ce qui écoute sur le port.
    if let Some(p) = q.strip_prefix(':').and_then(|p| p.trim().parse::<u16>().ok()) {
        return port(p);
    }

    let mut v = vec![];
    if let Some(c) = calc::eval(q, cx.taux) {
        let mut r = Res::new(Ico::Calc, c.display.clone(), "Entrée copie le résultat", Action::Copier(c.copy.clone()));
        r.couleur = c.color;
        v.push(r);
    }
    if let Some(s) = cx.sel {
        let sel: Vec<(i32, Res)> = selection(s, cx).into_iter().filter_map(|r| socle::fuzzy::score(&r.titre, q).map(|s| (s + 100, r))).collect();
        v.extend(trier(sel, q, cx));
    }
    v.extend(trier(chercher(q, cx, None, 8), q, cx).into_iter().take(8));
    // Fichiers et dossiers : en fond, après le reste.
    if q.chars().count() >= 2 {
        v.extend(asyncs(cx, &Req::Fichiers(brut.trim().to_string()), reqs));
    }
    v
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "'"))
}

fn port(p: u16) -> Vec<Res> {
    let l = crate::sysw::listeners(p);
    if l.is_empty() {
        return vec![Res::new(Ico::Web, format!("Rien n'écoute sur le port {p}"), "", Action::Rien)];
    }
    let mut v = vec![web(&format!("Ouvrir http://localhost:{p}"), "Navigateur", format!("http://localhost:{p}"))];
    for x in l {
        let Some(exe) = x.exe else {
            v.push(Res::new(Ico::Tuer, format!("{} (pid {})", x.name, x.pid), "Processus protégé : impossible à tuer d'ici", Action::Rien));
            continue;
        };
        v.push(Res::new(Ico::Tuer, format!("Tuer {} (pid {})", x.name, x.pid), exe.to_string_lossy(), Action::Tuer { pid: x.pid, exe: exe.to_string_lossy().into_owned() }).confirmer());
    }
    v
}

/// `k <nom|:port>` : serveurs de dev vus par Vigie (si elle sait), sinon le port en direct.
fn tuer(reste: &str, cx: &Ctx, reqs: &mut Vec<Req>) -> Vec<Res> {
    let port_q = reste.strip_prefix(':').and_then(|p| p.parse::<u16>().ok());
    if cx.apps.vigie_dev {
        let v = asyncs(cx, &Req::VigieDev, reqs);
        if v.is_empty() && !reqs.is_empty() {
            return vec![Res::new(Ico::Tuer, "Vigie : lecture des serveurs…", "", Action::Rien)];
        }
        let f: Vec<Res> = v
            .into_iter()
            .filter(|r| match port_q {
                Some(p) => r.sous.split(|c: char| !c.is_ascii_digit()).any(|x| x == p.to_string()),
                None => reste.is_empty() || socle::fuzzy::score(&format!("{} {}", r.titre, r.sous), reste).is_some(),
            })
            .collect();
        if !f.is_empty() {
            return f;
        }
    }
    match port_q {
        Some(p) => port(p).into_iter().filter(|r| matches!(r.action, Action::Tuer { .. })).collect(),
        None => vec![Res::new(Ico::Tuer, "Aucun serveur de dev trouvé", if cx.apps.vigie_dev { "d'après Vigie" } else { "Vigie absente : tape « k :port »" }, Action::Rien)],
    }
}

/// `t` : Ma journée ; `t <texte>` : ajouter (avec l'aperçu de ce que Tâches comprend) et chercher.
fn taches(reste: &str, cx: &Ctx, reqs: &mut Vec<Req>) -> Vec<Res> {
    let Some(exe) = &cx.apps.taches else { return vec![] };
    let e = exe.to_string_lossy().into_owned();
    let mut v = vec![];
    if !reste.is_empty() {
        let apercu = asyncs(cx, &Req::TachesComprendre(reste.to_string()), reqs);
        let (titre, sous) = match apercu.first() {
            Some(a) => (format!("Ajouter : {}", a.titre), a.sous.clone()),
            None => (format!("Ajouter : {reste}"), "…".to_string()),
        };
        let mut r = Res::new(Ico::Tache, titre, sous, Action::Sortie { exe: e.clone(), args: vec!["--ajouter".into(), reste.to_string()] });
        r.section = Some("Nouvelle tâche".into());
        r.alt = Some(Action::Ouvrir { cible: e.clone(), args: None, dossier: None });
        v.push(r);
    }
    let liste = asyncs(cx, &Req::TachesLister((!reste.is_empty()).then(|| reste.to_string())), reqs);
    if reste.is_empty() && liste.is_empty() && reqs.is_empty() {
        v.push(Res::new(Ico::Tache, "Ma journée est vide", "Tape « t » puis ta tâche pour l'ajouter", Action::Rien));
    }
    v.extend(liste);
    v
}

/// `d <projet> [t|gh]` : VS Code, terminal, GitHub.
fn dev(reste: &str, cx: &Ctx) -> Vec<Res> {
    let (nom, mode) = match reste.rsplit_once(' ') {
        Some((n, m)) if m == "t" || m == "gh" => (n.trim(), m),
        _ => (reste, ""),
    };
    let noms: Vec<String> = cx.projets.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
    let mut s: Vec<(i32, usize)> = noms.iter().enumerate().filter_map(|(i, n)| if nom.is_empty() { Some((cx.appris.bonus(&format!("dev:{n}")), i)) } else { socle::fuzzy::score(n, nom).map(|s| (s + cx.appris.bonus(&format!("dev:{n}")), i)) }).collect();
    s.sort_by(|a, b| b.0.cmp(&a.0));
    let mut v = vec![];
    for (_, i) in s.into_iter().take(8) {
        let p = &cx.projets[i];
        let ps = p.to_string_lossy().into_owned();
        let n = &noms[i];
        let r = match mode {
            "t" => {
                let (cible, args) = match &cx.apps.wt {
                    Some(wt) => (wt.to_string_lossy().into_owned(), Some(format!("-d {}", quote(&ps)))),
                    None => ("powershell.exe".to_string(), None),
                };
                Res::new(Ico::Terminal, format!("{n} : terminal"), &ps, Action::Ouvrir { cible, args, dossier: Some(ps.clone()) })
            }
            "gh" => match crate::sources::github(p) {
                Some(u) => web(&format!("{n} : GitHub"), &u, u.clone()),
                None => Res::new(Ico::Git, format!("{n} : pas de dépôt GitHub"), "remote « origin » absent", Action::Rien),
            },
            _ => match &cx.apps.code {
                Some(code) => Res::new(Ico::Code, format!("{n} : VS Code"), &ps, Action::Ouvrir { cible: code.to_string_lossy().into_owned(), args: Some(quote(&ps)), dossier: None }),
                None => Res::new(Ico::Dossier, format!("{n} : dossier"), format!("{ps} (VS Code introuvable)"), Action::Ouvrir { cible: ps.clone(), args: None, dossier: None }),
            },
        };
        let mut r = r.cle(format!("dev:{n}"));
        r.alt = Some(Action::Ouvrir { cible: ps, args: None, dossier: None });
        v.push(r);
    }
    if v.is_empty() {
        v.push(Res::new(Ico::Code, format!("Aucun projet « {nom} »"), crate::sources::racine_dev().to_string_lossy(), Action::Rien));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Monde {
        items: Vec<Item>,
        index: socle::fuzzy::Index,
        appris: Appris,
        taux: calc::Rates,
        snippets: BTreeMap<String, String>,
        projets: Vec<PathBuf>,
        apps: Apps,
        asyncs: HashMap<Req, Vec<Res>>,
    }

    fn monde() -> Monde {
        let items = vec![
            Item { genre: Genre::App, nom: "Google Chrome".into(), cible: "C:\\x\\Chrome.lnk".into(), detail: "App".into(), img: None },
            Item { genre: Genre::App, nom: "Calculatrice".into(), cible: "C:\\x\\Calc.lnk".into(), detail: "App".into(), img: None },
            Item { genre: Genre::Jeu, nom: "Hades".into(), cible: "steam://rungameid/1145360".into(), detail: "Jeu".into(), img: None },
            Item { genre: Genre::Maison, nom: "Explorateur".into(), cible: "C:\\Dev\\explorateur\\dist\\Explorateur.exe".into(), detail: "App maison".into(), img: None },
        ];
        let index = socle::fuzzy::Index::from_iter(items.iter().map(|i| i.nom.as_str()));
        let mut snippets = BTreeMap::new();
        snippets.insert("mail".to_string(), "moi@exemple.fr".to_string());
        Monde { items, index, appris: Appris::default(), taux: calc::Rates::default(), snippets, projets: vec![PathBuf::from("C:\\Dev\\lanceur"), PathBuf::from("C:\\Dev\\vigie")], apps: Apps::default(), asyncs: HashMap::new() }
    }

    fn cx<'a>(m: &'a Monde, derniere: Option<&'a Res>) -> Ctx<'a> {
        Ctx { items: &m.items, index: &m.index, appris: &m.appris, taux: &m.taux, snippets: &m.snippets, projets: &m.projets, apps: &m.apps, sel: None, derniere, asyncs: &m.asyncs }
    }

    #[test]
    fn deux_lettres_trouvent_l_app() {
        let m = monde();
        let (r, reqs) = requete("ch", &cx(&m, None));
        assert_eq!(r[0].titre, "Google Chrome");
        assert_eq!(reqs, vec![Req::Fichiers("ch".into())]);
    }

    #[test]
    fn apprentissage_et_requete_exacte() {
        let mut m = monde();
        m.appris.noter("ca", "C:\\x\\Calc.lnk");
        let (r, _) = requete("ca", &cx(&m, None));
        assert_eq!(r[0].titre, "Calculatrice");
        assert!(m.appris.bonus("C:\\x\\Calc.lnk") > 0);
    }

    #[test]
    fn mauvais_clavier() {
        let m = monde();
        // « hqdes » : Hades tapé en QWERTY sur un clavier AZERTY.
        let (r, _) = requete("hqdes", &cx(&m, None));
        assert!(r.iter().any(|x| x.titre == "Hades"));
    }

    #[test]
    fn prefixes() {
        let m = monde();
        let c = cx(&m, None);
        let (r, _) = requete("g rust egui", &c);
        assert_eq!(r[0].action, Action::Ouvrir { cible: "https://www.google.com/search?q=rust+egui".into(), args: None, dossier: None });
        let (r, _) = requete("p had", &c);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].titre, "Hades");
        let (r, _) = requete(";ma", &c);
        assert_eq!(r[0].action, Action::Copier("moi@exemple.fr".into()));
        let (r, _) = requete("d lanc", &c);
        assert!(r[0].titre.starts_with("lanceur"));
        let (r, _) = requete("d vig t", &c);
        assert!(r[0].titre.starts_with("vigie : terminal"));
    }

    #[test]
    fn apps_absentes_masquees() {
        let m = monde();
        let c = cx(&m, None);
        // Sans Tube, « y » n'est pas une commande : recherche normale.
        let (r, _) = requete("y hades", &c);
        assert!(r.iter().all(|x| !x.titre.starts_with("Tube")));
        let (r, reqs) = requete("v mot", &c);
        assert!(r.iter().all(|x| x.ico != Ico::Memoire) && !reqs.contains(&Req::Memoire("mot".into())));
        let (r, _) = requete("mode jeu", &c);
        assert!(r.iter().all(|x| x.titre != "Mode jeu"));
    }

    #[test]
    fn tube_present() {
        let mut m = monde();
        m.apps.tube = Some(PathBuf::from("C:\\Dev\\tube\\dist\\Tube.exe"));
        let (r, _) = requete("y lofi \"girl\"", &cx(&m, None));
        assert_eq!(r[0].action, Action::Ouvrir { cible: "C:\\Dev\\tube\\dist\\Tube.exe".into(), args: Some("\"lofi 'girl'\"".into()), dossier: None });
    }

    #[test]
    fn repeter_et_calcul() {
        let m = monde();
        let d = Res::new(Ico::App, "Hades", "Jeu", Action::Ouvrir { cible: "steam://rungameid/1145360".into(), args: None, dossier: None });
        let (r, _) = requete("!", &cx(&m, Some(&d)));
        assert_eq!(r[0].titre, "Hades");
        let (r, _) = requete("2+3*4", &cx(&m, None));
        assert_eq!(r[0].action, Action::Copier("14".into()));
    }

    #[test]
    fn selection_json() {
        let m = monde();
        let mut c = cx(&m, None);
        c.sel = Some("{\"a\":1}");
        let (r, _) = requete("", &c);
        assert!(r.iter().any(|x| x.titre.starts_with("JSON : formater")));
        assert!(r.iter().any(|x| x.titre == "Traduire en français"));
    }

    #[test]
    fn taches_ajout_et_liste() {
        let mut m = monde();
        let c = cx(&m, None);
        // Sans le marqueur : « t » n'est pas un préfixe.
        let (r, _) = requete("t beurre", &c);
        assert!(r.iter().all(|x| x.ico != Ico::Tache));
        m.apps.taches = Some(PathBuf::from("C:\\Dev\\taches\\dist\\Taches.exe"));
        m.apps.taches_cli = true;
        let (r, reqs) = requete("t beurre demain 18h #courses !", &cx(&m, None));
        assert_eq!(r[0].action, Action::Sortie { exe: "C:\\Dev\\taches\\dist\\Taches.exe".into(), args: vec!["--ajouter".into(), "beurre demain 18h #courses !".into()] });
        assert!(reqs.contains(&Req::TachesComprendre("beurre demain 18h #courses !".into())));
        m.asyncs.insert(Req::TachesComprendre("beurre".into()), vec![Res::new(Ico::Tache, "beurre", "courses · demain 18:00 · ★", Action::Rien)]);
        let (r, _) = requete("t beurre", &cx(&m, None));
        assert_eq!(r[0].titre, "Ajouter : beurre");
        assert_eq!(r[0].sous, "courses · demain 18:00 · ★");
        let (_, reqs) = requete("t", &cx(&m, None));
        assert_eq!(reqs, vec![Req::TachesLister(None)]);
    }

    #[test]
    fn port_vide() {
        // Port très probablement libre : la requête ne doit rien proposer à tuer.
        let m = monde();
        let (r, _) = requete(":1", &cx(&m, None));
        assert!(r.iter().all(|x| !matches!(x.action, Action::Tuer { .. })));
    }
}
