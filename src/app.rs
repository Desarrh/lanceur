//! État et dessin de la barre. Le dessin renvoie des actions, exécutées ensuite sur un thread.
use crate::integr;
use crate::moteur::{self, Action, Appris, Apps, Ico, Req, Res, Sys};
use crate::sources::{self, Fichiers, Item};
use egui::{Color32, FontId, Key, Modifiers, Rect, Sense, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub const APP: &str = "Lanceur";
pub const LARGEUR: f32 = 680.0;
const CHAMP: f32 = 60.0;
const LIGNE: f32 = 48.0;
const LIGNE_CALC: f32 = 66.0;
const SECTION: f32 = 26.0;
const PUCES: f32 = 46.0;
const PIED: f32 = 30.0;
/// Hauteur maximale de la liste (au-delà : défilement qui suit la sélection).
const LISTE_MAX: f32 = 470.0;

/// Préfixes affichés en puce : (tête tapée, libellé, icône).
const PREFIXES: &[(&str, &str, socle::icons::Icon)] = &[
    ("p", "Jeux", socle::icons::Icon::Gamepad),
    ("d", "Projets", socle::icons::Icon::Code),
    ("y", "Tube", socle::icons::Icon::Play),
    ("v", "Mémoire", socle::icons::Icon::History),
    ("t", "Tâches", socle::icons::Icon::Check),
    ("k", "Vigie", socle::icons::Icon::Bolt),
    ("c", "Claude", socle::icons::Icon::Terminal),
    ("g", "Google", socle::icons::Icon::Link),
    ("yt", "YouTube", socle::icons::Icon::Play),
    ("gh", "GitHub", socle::icons::Icon::Git),
];

/// Ligne affichée : en-tête de section ou résultat.
enum Ligne {
    Section(String),
    Res(usize),
}

/// Section d'un résultat : imposée par le moteur, sinon déduite de son icône.
fn section(r: &Res) -> &str {
    if let Some(s) = &r.section {
        return s;
    }
    match r.ico {
        Ico::Calc => "Calcul",
        Ico::App | Ico::Maison => "Apps",
        Ico::Jeu => "Jeux",
        Ico::Dossier | Ico::Fichier => "Fichiers",
        Ico::Code | Ico::Terminal | Ico::Git if r.cle.as_deref().is_some_and(|c| c.starts_with("dev:")) => "Projets",
        Ico::Claude => "Sessions Claude",
        Ico::Memoire => "Mémoire",
        Ico::Tache => "Tâches",
        Ico::Video => "Tube",
        Ico::Web => "Web",
        _ => "Actions",
    }
}

/// Clé d'image → pixels (thread des icônes).
fn charger_icone(k: &str, jaq: &std::path::Path, steam: Option<&std::path::Path>) -> Option<crate::icones::Img> {
    if let Some(chemin) = k.strip_prefix("p:") {
        return crate::icones::icone(chemin, 32);
    }
    if let Some(ext) = k.strip_prefix("t:") {
        return crate::icones::icone_type(ext, ext == "/", 32);
    }
    let rest = k.strip_prefix("j:")?;
    let mut it = rest.splitn(3, '|');
    let (cle, appid, url) = (it.next()?, it.next()?.parse::<u64>().ok(), it.next().filter(|u| !u.is_empty()));
    crate::icones::jaquette(cle, url, steam, appid, jaq, 52, 76)
}

pub static UPDATER: socle::update::Updater = socle::update::Updater { repo: "Desarrh/lanceur", current: env!("CARGO_PKG_VERSION"), asset_suffix: "windows-x64.zip", exe_name: Some("Lanceur.exe") };

fn vrai() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
pub struct Settings {
    /// Démarrer avec Windows (entrée Run écrite au lancement).
    #[serde(default = "vrai")]
    pub demarrage: bool,
    #[serde(default)]
    pub appris: Appris,
    /// `;nom` → texte copié.
    #[serde(default)]
    pub snippets: BTreeMap<String, String>,
    #[serde(default)]
    pub derniere: Option<Res>,
    #[serde(default)]
    pub last_update_check: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let mut snippets = BTreeMap::new();
        snippets.insert("exemple".into(), "Texte copié par ;exemple — modifie la liste dans « Paramètres du Lanceur ».".into());
        Settings { demarrage: true, appris: Appris::default(), snippets, derniere: None, last_update_check: 0 }
    }
}

impl Settings {
    pub fn load() -> Self {
        socle::settings::load(APP)
    }
}

/// Changement demandé depuis le menu de l'icône (autre thread) : 0 rien, 1 activé, 2 désactivé.
static DEMARRAGE: AtomicU8 = AtomicU8::new(0);
pub fn set_demarrage(on: bool) {
    DEMARRAGE.store(if on { 1 } else { 2 }, Ordering::Relaxed);
}

/// Ce qui se lit lentement (apps, jeux, projets, apps maison, taux).
pub struct Donnees {
    pub items: Vec<Item>,
    pub index: socle::fuzzy::Index,
    pub projets: Vec<std::path::PathBuf>,
    pub apps: Apps,
    pub taux: crate::calc::Rates,
}

pub fn charger(appris: &Appris, reseau: bool) -> Donnees {
    let items = sources::items();
    let mut index = socle::fuzzy::Index::new();
    for it in &items {
        index.push(&it.nom, 0);
    }
    let _ = appris; // le bonus d'usage est ajouté au moment de la recherche
    Donnees { items, index, projets: sources::projets(), apps: integr::detecter(), taux: integr::taux(reseau) }
}

enum Evt {
    Donnees(Arc<Donnees>),
    Fichiers(Arc<Fichiers>, Option<std::time::SystemTime>),
    Async(Req, Vec<Res>),
    Fait(Result<Option<String>, String>),
    Maj(Option<socle::update::Release>),
    Icone(String, Option<crate::icones::Img>),
}

pub struct App {
    ctx: egui::Context,
    pub pal: socle::theme::Palette,
    pub s: Settings,
    saver: socle::settings::Saver,
    d: Arc<Donnees>,
    charge_le: Instant,
    fichiers: Option<Arc<Fichiers>>,
    fichiers_date: Option<std::time::SystemTime>,
    q: String,
    q_calc: Option<String>,
    res: Vec<Res>,
    i: usize,
    confirm: Option<usize>,
    sel: Option<String>,
    msg: Option<(String, bool)>,
    asyncs: HashMap<Req, Vec<Res>>,
    en_cours: HashSet<Req>,
    tx: Sender<Evt>,
    rx: Receiver<Evt>,
    maj: Option<socle::update::Release>,
    /// La barre doit se cacher (lu par la boucle principale).
    pub cacher: bool,
    pub quitter: bool,
    focus: bool,
    /// Préfixe affiché en puce dans le champ (`p`, `y`…) ; `q` ne contient alors que le reste.
    prefixe: Option<&'static str>,
    /// Icônes Windows et jaquettes : clé `Res::img` → texture (None = en cours ou introuvable).
    icones: HashMap<String, Option<egui::TextureHandle>>,
    tx_ico: Sender<String>,
    /// Résumé de Bilan (barre vide), avec son heure.
    bilan: Option<(String, Instant)>,
}

impl App {
    pub fn new(ctx: &egui::Context, s: Settings, d: Donnees) -> App {
        socle::theme::install_fonts(ctx);
        let pal = socle::theme::palette(socle::theme::system_dark(ctx));
        socle::theme::apply(ctx, &pal);
        let (tx, rx) = channel();
        // Un seul thread pour les icônes (COM, cache disque des jaquettes) : rien ne bloque l'interface.
        let (tx_ico, rx_ico) = channel::<String>();
        {
            let (tx, c) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let jaq = socle::paths::cache_dir(APP).join("jaquettes");
                let _ = std::fs::create_dir_all(&jaq);
                let steam = sources::steam_path();
                while let Ok(k) = rx_ico.recv() {
                    let img = std::panic::catch_unwind(|| charger_icone(&k, &jaq, steam.as_deref())).ok().flatten();
                    let _ = tx.send(Evt::Icone(k, img));
                    c.request_repaint();
                }
            });
        }
        let mut app = App {
            ctx: ctx.clone(),
            pal,
            s,
            saver: socle::settings::Saver::debounced(Duration::from_millis(400), Duration::from_secs(3)),
            d: Arc::new(d),
            charge_le: Instant::now(),
            fichiers: None,
            fichiers_date: None,
            q: String::new(),
            q_calc: None,
            res: vec![],
            i: 0,
            confirm: None,
            sel: None,
            msg: socle::crash::pending(APP).map(|_| ("Le Lanceur a planté la dernière fois (crash.log dans ses paramètres)".to_string(), true)),
            asyncs: HashMap::new(),
            en_cours: HashSet::new(),
            tx,
            rx,
            maj: None,
            cacher: false,
            quitter: false,
            focus: true,
            prefixe: None,
            icones: HashMap::new(),
            tx_ico,
            bilan: None,
        };
        app.charger_fichiers();
        app.recharger(); // taux de change du jour, en fond
        if socle::update::due(app.s.last_update_check) && !socle::testmode::enabled() {
            app.s.last_update_check = moteur::maintenant();
            app.saver.touch();
            app.fond(move || Evt::Maj(UPDATER.check().ok().flatten()));
        }
        app
    }

    fn fond(&self, f: impl FnOnce() -> Evt + Send + 'static) {
        let (tx, c) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Ok(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                let _ = tx.send(e);
            }
            c.request_repaint();
        });
    }

    /// Index des fichiers : celui de l'Explorateur s'il existe (relu s'il a changé), sinon parcours à nous.
    fn charger_fichiers(&mut self) {
        let f = sources::index_explorateur();
        let date = f.as_ref().and_then(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
        if self.fichiers.is_some() && (date.is_none() || date == self.fichiers_date) {
            return;
        }
        self.fichiers_date = date; // évite deux chargements en parallèle
        self.fond(move || {
            let fx = f.as_deref().filter(|_| date.is_some()).and_then(sources::lire_explorateur).unwrap_or_else(sources::parcourir);
            Evt::Fichiers(Arc::new(fx), date)
        });
    }

    fn recharger(&mut self) {
        let appris = self.s.appris.clone();
        self.charge_le = Instant::now();
        self.fond(move || Evt::Donnees(Arc::new(charger(&appris, true))));
    }

    /// Ouverture par Alt+Espace : barre vide, sélection éventuelle, données rafraîchies si vieilles.
    pub fn ouvrir(&mut self, sel: Option<String>) {
        self.q.clear();
        self.prefixe = None;
        if self.d.apps.bilan_resume && self.bilan.as_ref().is_none_or(|b| b.1.elapsed() > Duration::from_secs(600)) {
            self.bilan = Some((String::new(), Instant::now()));
            let apps = self.d.apps.clone();
            self.fond(move || Evt::Async(Req::Bilan, integr::faire(&Req::Bilan, &apps)));
        }
        self.sel = sel.filter(|s| s.chars().count() <= 20_000);
        self.asyncs.clear();
        self.msg = self.msg.take().filter(|m| m.1);
        self.focus = true;
        self.calculer();
        if self.charge_le.elapsed() > Duration::from_secs(600) {
            self.recharger();
        }
        self.charger_fichiers();
    }

    pub fn set_selection(&mut self, s: String) {
        self.sel = Some(s);
        self.calculer();
    }

    /// Texte complet de la requête (préfixe en puce + saisie).
    fn texte(&self) -> String {
        match self.prefixe {
            Some(p) => format!("{p} {}", self.q),
            None => self.q.clone(),
        }
    }

    fn vide(&self) -> bool {
        self.prefixe.is_none() && self.q.trim().is_empty()
    }

    fn dispo(&self, t: &str) -> bool {
        let a = &self.d.apps;
        match t {
            "y" => a.tube.is_some(),
            "v" => a.memoire_cherche,
            "c" => a.claude.is_some(),
            "t" => a.taches_cli,
            _ => true,
        }
    }

    /// `p hades` tapé en entier : la tête passe en puce si l'app correspondante est là.
    fn normaliser(&mut self) {
        if self.prefixe.is_some() {
            return;
        }
        let Some((tete, reste)) = self.q.split_once(' ') else { return };
        let tete = tete.to_lowercase();
        if let Some(&(t, _, _)) = PREFIXES.iter().find(|x| x.0 == tete && self.dispo(x.0)) {
            self.prefixe = Some(t);
            self.q = reste.to_string();
        }
    }

    fn puces(&self) -> Vec<(&'static str, &'static str, socle::icons::Icon)> {
        let mut v: Vec<_> = PREFIXES.iter().copied().filter(|x| !matches!(x.0, "yt" | "gh") && (x.0 != "k" || self.d.apps.vigie.is_some()) && self.dispo(x.0)).collect();
        v.push((";", "Snippets", socle::icons::Icon::Copy));
        v
    }

    fn lignes(&self) -> Vec<(Ligne, f32)> {
        let mut v = vec![];
        let mut sec = "";
        for (i, r) in self.res.iter().enumerate() {
            let s = section(r);
            if s != sec {
                v.push((Ligne::Section(s.to_string()), SECTION));
                sec = s;
            }
            v.push((Ligne::Res(i), if r.ico == Ico::Calc && !self.vide() { LIGNE_CALC } else { LIGNE }));
        }
        v
    }

    fn bilan_ligne(&self) -> Option<&str> {
        self.bilan.as_ref().map(|b| b.0.as_str()).filter(|s| !s.is_empty() && self.vide())
    }

    pub fn hauteur(&self) -> f32 {
        let liste: f32 = self.lignes().iter().map(|x| x.1).sum::<f32>().min(LISTE_MAX);
        let puces = if self.vide() { PUCES + if self.bilan_ligne().is_some() { 26.0 } else { 0.0 } } else { 0.0 };
        CHAMP + puces + if liste > 0.0 { liste + 8.0 } else { 0.0 } + PIED
    }

    fn calculer(&mut self) {
        let t0 = Instant::now();
        let (mut res, reqs) = {
            let d = &self.d;
            let cx = moteur::Ctx {
                items: &d.items,
                index: &d.index,
                appris: &self.s.appris,
                taux: &d.taux,
                snippets: &self.s.snippets,
                projets: &d.projets,
                apps: &d.apps,
                sel: self.sel.as_deref(),
                derniere: self.s.derniere.as_ref(),
                asyncs: &self.asyncs,
            };
            moteur::requete(&self.texte(), &cx)
        };
        // Regroupe par section, dans l'ordre de première apparition (le meilleur reste en tête).
        let mut ordre: Vec<String> = vec![];
        for r in &res {
            let s = section(r).to_string();
            if !ordre.contains(&s) {
                ordre.push(s);
            }
        }
        res.sort_by_key(|r| ordre.iter().position(|s| s == section(r)));
        if self.vide() && let Some(m) = &self.maj {
            res.insert(0, Res::new(Ico::Repeter, format!("Mettre à jour le Lanceur ({})", m.version), "Entrée : installe et relance", Action::Rien).cle("maj"));
        }
        let texte = self.texte();
        if self.q_calc.as_deref() != Some(texte.as_str()) {
            self.i = 0;
            self.confirm = None;
        }
        if std::env::var_os("LANCEUR_MESURE").is_some() {
            eprintln!("recherche « {texte} » : {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
        }
        self.q_calc = Some(texte);
        for r in &res {
            if let Some(k) = &r.img
                && !self.icones.contains_key(k)
            {
                self.icones.insert(k.clone(), None);
                let _ = self.tx_ico.send(k.clone());
            }
        }
        self.i = self.i.min(res.len().saturating_sub(1));
        self.res = res;
        for r in reqs {
            if !self.en_cours.insert(r.clone()) {
                continue;
            }
            match &r {
                Req::Fichiers(q) => {
                    let Some(fx) = self.fichiers.clone() else {
                        self.en_cours.remove(&r);
                        continue;
                    };
                    let (q, appris) = (q.clone(), self.s.appris.usage.clone());
                    self.fond(move || {
                        let t0 = Instant::now();
                        let mut v: Vec<(i32, Res)> = fx
                            .index
                            .search(&q, 8)
                            .into_iter()
                            .map(|h| {
                                let p = fx.chemin(h.index);
                                let ps = p.to_string_lossy().into_owned();
                                let dir = fx.dossier[h.index];
                                let parent = p.parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                                let mut r = Res::new(if dir { Ico::Dossier } else { Ico::Fichier }, fx.nom(h.index), parent, Action::Ouvrir { cible: ps.clone(), args: None, dossier: None }).cle(ps.clone());
                                r.alt = Some(Action::Reveler(ps.clone()));
                                r.img = Some(if crate::icones::icone_propre(&ps) {
                                    format!("p:{ps}")
                                } else if dir {
                                    "t:/".to_string()
                                } else {
                                    format!("t:{}", p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default())
                                });
                                let b = appris.get(&ps).map_or(0, |(n, _)| (*n).min(30) as i32 * 10);
                                (h.score + b, r)
                            })
                            .collect();
                        v.sort_by(|a, b| b.0.cmp(&a.0));
                        if std::env::var_os("LANCEUR_MESURE").is_some() {
                            eprintln!("fichiers « {q} » ({} entrées) : {:.1} ms", fx.len(), t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        Evt::Async(r, v.into_iter().map(|x| x.1).collect())
                    });
                }
                _ => {
                    let apps = self.d.apps.clone();
                    self.fond(move || {
                        let v = integr::faire(&r, &apps);
                        Evt::Async(r, v)
                    });
                }
            }
        }
    }

    /// Événements des threads de fond.
    pub fn tick(&mut self) {
        let mut changer = false;
        while let Ok(e) = self.rx.try_recv() {
            match e {
                Evt::Donnees(d) => {
                    self.d = d;
                    changer = true;
                }
                Evt::Fichiers(f, date) => {
                    self.fichiers = Some(f);
                    self.fichiers_date = date;
                    self.asyncs.retain(|k, _| !matches!(k, Req::Fichiers(_)));
                    self.en_cours.retain(|k| !matches!(k, Req::Fichiers(_)));
                    changer = true;
                }
                Evt::Async(Req::Bilan, v) if self.vide() || !self.asyncs.contains_key(&Req::Bilan) => {
                    let l = v.first().filter(|r| matches!(r.action, Action::Copier(_))).map(|r| r.titre.clone()).unwrap_or_default();
                    self.bilan = Some((l, Instant::now()));
                    self.en_cours.remove(&Req::Bilan);
                    self.asyncs.insert(Req::Bilan, v);
                    changer = true;
                }
                Evt::Icone(k, img) => {
                    let t = img.map(|i| self.ctx.load_texture(&k, egui::ColorImage::from_rgba_unmultiplied([i.w as usize, i.h as usize], &i.rgba), egui::TextureOptions::LINEAR));
                    self.icones.insert(k, t);
                }
                Evt::Async(r, v) => {
                    self.en_cours.remove(&r);
                    if matches!(r, Req::Fichiers(_) | Req::Memoire(_) | Req::TachesComprendre(_) | Req::TachesLister(Some(_))) {
                        let meme = std::mem::discriminant(&r);
                        self.asyncs.retain(|k, _| std::mem::discriminant(k) != meme);
                    }
                    self.asyncs.insert(r, v);
                    changer = true;
                }
                Evt::Fait(r) => {
                    match r {
                        Ok(Some(m)) => self.msg = Some((m, false)),
                        Ok(None) => {}
                        Err(e) => self.msg = Some((e, true)),
                    }
                    if matches!(self.res.get(self.i).map(|r| &r.action), Some(Action::Sortie { .. })) {
                        self.asyncs.clear(); // l'état a changé (serveurs coupés…)
                        changer = true;
                    }
                }
                Evt::Maj(m) => {
                    self.maj = m;
                    changer = true;
                }
            }
        }
        match DEMARRAGE.swap(0, Ordering::Relaxed) {
            1 => {
                self.s.demarrage = true;
                self.saver.touch();
            }
            2 => {
                self.s.demarrage = false;
                self.saver.touch();
            }
            _ => {}
        }
        if changer {
            self.calculer();
        }
        if let Some(wait) = self.saver.tick(APP, &self.s) {
            self.ctx.request_repaint_after(wait);
        }
        if let Some(e) = self.saver.take_error() {
            self.msg = Some((e, true));
        }
    }

    pub fn flush(&mut self) {
        let _ = self.saver.flush(APP, &self.s);
    }

    /// Exécute le résultat `i` (`alt` = Maj+Entrée).
    fn choisir(&mut self, i: usize, alt: bool) {
        let Some(r) = self.res.get(i).cloned() else { return };
        let action = if alt { r.alt.clone().unwrap_or(r.action.clone()) } else { r.action.clone() };
        if r.cle.as_deref() == Some("maj") {
            if let Some(m) = self.maj.clone() {
                self.msg = Some(("Mise à jour en cours…".into(), false));
                self.fond(move || Evt::Fait(UPDATER.install(&m, &|_| {}).and_then(|exe| socle::update::relaunch(&exe)).map(|_| {
                    std::process::exit(0);
                })));
            }
            return;
        }
        if r.confirmer && self.confirm != Some(i) {
            self.confirm = Some(i);
            self.msg = Some((format!("Entrée encore pour confirmer : {}", r.titre), false));
            return;
        }
        self.confirm = None;
        match &action {
            Action::Rien => return,
            Action::Remplir(t) => {
                self.q = t.clone();
                self.prefixe = None;
                self.normaliser();
                self.focus = true;
                self.calculer();
                return;
            }
            Action::Sys(Sys::Recharger) => {
                self.recharger();
                self.fichiers_date = None;
                self.fichiers = None;
                self.charger_fichiers();
                self.msg = Some(("Index rechargé en arrière-plan".into(), false));
                return;
            }
            Action::Sys(Sys::Reglages) => {
                self.flush();
                let p = socle::settings::path(APP);
                let _ = crate::sysw::open(&p.to_string_lossy(), None, None);
                self.cacher = true;
                return;
            }
            Action::Sys(Sys::Quitter) => {
                self.quitter = true;
                return;
            }
            _ => {}
        }
        if let Some(c) = &r.cle {
            let t = self.texte();
            self.s.appris.noter(&t, c);
        }
        let mut derniere = r.clone();
        derniere.action = action.clone();
        derniere.alt = None;
        self.s.derniere = Some(derniere);
        self.saver.touch();
        let garder = matches!(action, Action::Sortie { .. });
        if garder {
            self.msg = Some(("…".into(), false));
            if let Action::Sortie { args, .. } = &action
                && args.first().map(String::as_str) == Some("--ajouter")
            {
                self.q.clear(); // la puce « Tâches » reste : on enchaîne la suivante
                self.calculer();
            }
        } else {
            self.cacher = true;
        }
        self.fond(move || Evt::Fait(integr::executer(&action)));
    }

    /// Sans interface (`--action`) : exécute le premier résultat, en attendant le travail lent.
    pub fn action_directe(d: &Donnees, s: &mut Settings, q: &str) -> Result<Option<String>, String> {
        let mut asyncs = HashMap::new();
        for _ in 0..2 {
            let cx = moteur::Ctx { items: &d.items, index: &d.index, appris: &s.appris, taux: &d.taux, snippets: &s.snippets, projets: &d.projets, apps: &d.apps, sel: None, derniere: s.derniere.as_ref(), asyncs: &asyncs };
            let (res, reqs) = moteur::requete(q, &cx);
            let lent: Vec<Req> = reqs.into_iter().filter(|r| !matches!(r, Req::Fichiers(_))).collect();
            if lent.is_empty() || res.iter().any(|r| r.action != Action::Rien) {
                let r = res.into_iter().find(|r| r.action != Action::Rien).ok_or_else(|| format!("rien pour « {q} »"))?;
                if let Some(c) = &r.cle {
                    s.appris.noter(q, c);
                }
                if let Action::Remplir(t) = &r.action {
                    return App::action_directe(d, s, t);
                }
                s.derniere = Some(r.clone());
                return integr::executer(&r.action);
            }
            for r in lent {
                let v = integr::faire(&r, &d.apps);
                asyncs.insert(r, v);
            }
        }
        Err(format!("rien pour « {q} »"))
    }

    pub fn handle_keys(&mut self) {
        let n = self.res.len();
        let retour_vide = self.q.is_empty() && self.prefixe.is_some();
        let (bas, haut, entree, maj_entree, echap, tab, retour) = self.ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::SHIFT, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Tab),
                retour_vide && i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if bas && n > 0 {
            self.i = (self.i + 1) % n;
            self.confirm = None;
        }
        if haut && n > 0 {
            self.i = (self.i + n - 1) % n;
            self.confirm = None;
        }
        if retour {
            self.prefixe = None;
            self.calculer();
        }
        if echap {
            if self.vide() {
                self.cacher = true;
            } else {
                self.q.clear();
                self.prefixe = None;
                self.calculer();
            }
        }
        if tab && let Some(r) = self.res.get(self.i) {
            // Complète avec le titre (utile pour « d proj », « p jeu »).
            let nom = r.titre.split(" : ").next().unwrap_or(&r.titre).to_string();
            self.q = if self.prefixe.is_some() { format!("{nom} ") } else { nom };
            self.calculer();
        }
        if entree {
            self.choisir(self.i, false);
        }
        if maj_entree {
            self.choisir(self.i, true);
        }
    }

    /// Icône d'un résultat : vraie icône Windows / jaquette si chargée, sinon icône du socle.
    fn peindre_icone(&self, ui: &Ui, r: &Res, centre: egui::Pos2, actif: bool) {
        let p = &self.pal;
        if let Some([r_, g, b]) = r.couleur {
            let rc = Rect::from_center_size(centre, vec2(26.0, 26.0));
            ui.painter().rect_filled(rc, 6.0, Color32::from_rgb(r_, g, b));
            ui.painter().rect_stroke(rc, 6.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Outside);
            return;
        }
        if let Some(Some(t)) = r.img.as_ref().and_then(|k| self.icones.get(k)) {
            let [w, h] = t.size();
            let taille = if h > w { vec2(26.0, 38.0) } else { vec2(26.0, 26.0) };
            let rc = Rect::from_center_size(centre, taille);
            egui::Image::new((t.id(), taille)).corner_radius(if h > w { 4.0 } else { 0.0 }).paint_at(ui, rc);
            return;
        }
        let fond = if actif { p.accent } else { p.muted };
        let rc = Rect::from_center_size(centre, vec2(30.0, 30.0));
        ui.painter().rect_filled(rc, 8.0, socle::theme::blend(p.surface, fond, 0.12));
        socle::icons::paint(ui.painter(), rc.shrink(6.0), icone(r.ico), fond);
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        let p = self.pal.clone();
        let plein = ui.max_rect();
        ui.painter().rect_filled(plein, 0.0, p.surface);

        // --- Champ : loupe, puce du préfixe, saisie ---
        let champ = Rect::from_min_size(plein.min, vec2(plein.width(), CHAMP));
        socle::icons::paint(ui.painter(), Rect::from_center_size(pos2(champ.left() + 30.0, champ.center().y), vec2(22.0, 22.0)), socle::icons::Icon::Search, p.accent);
        let mut x0 = champ.left() + 54.0;
        if let Some(pf) = self.prefixe {
            let (lib, ic) = PREFIXES.iter().find(|x| x.0 == pf).map_or(("", socle::icons::Icon::Dot), |x| (x.1, x.2));
            let g = ui.painter().layout_no_wrap(lib.to_string(), FontId::proportional(14.0), p.on_accent);
            let rc = Rect::from_min_size(pos2(x0, champ.center().y - 14.0), vec2(g.size().x + 40.0, 28.0));
            ui.painter().rect_filled(rc, 14.0, p.accent);
            socle::icons::paint(ui.painter(), Rect::from_center_size(pos2(rc.left() + 16.0, rc.center().y), vec2(16.0, 16.0)), ic, p.on_accent);
            ui.painter().galley(pos2(rc.left() + 30.0, rc.center().y - g.size().y / 2.0), g, p.on_accent);
            x0 = rc.right() + 10.0;
        }
        let edit_rect = Rect::from_min_max(pos2(x0, champ.top() + 14.0), pos2(champ.right() - 18.0, champ.bottom() - 14.0));
        let avant = self.q.clone();
        let aide = match self.prefixe {
            Some("p") => "Quel jeu ?",
            Some("d") => "Quel projet ? (ajoute « t » terminal, « gh » GitHub)",
            Some("y") => "Rechercher dans Tube",
            Some("v") => "Rechercher dans Mémoire",
            Some("k") => "Quel serveur ? nom ou :port",
            Some("c") => "Quelle session Claude ?",
            Some("t") => "Nouvelle tâche (demain 18h #liste !) ou recherche",
            Some(_) => "Rechercher sur le web",
            None if self.sel.is_some() => "Texte sélectionné : choisis une action, ou tape…",
            None => "Rechercher une app, un jeu, un fichier, calculer…",
        };
        let resp = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut self.q)
                .id_salt("q")
                .frame(egui::Frame::NONE)
                .font(FontId::proportional(20.0))
                .text_color(p.text)
                .hint_text(egui::RichText::new(aide).color(p.muted))
                .desired_width(edit_rect.width()),
        );
        if self.focus {
            resp.request_focus();
            self.focus = false;
        }
        if self.q != avant {
            self.normaliser();
            self.calculer();
        }
        let mut y = champ.bottom();
        ui.painter().hline(plein.x_range(), y, egui::Stroke::new(1.0, p.border));

        // --- Barre vide : puces des préfixes et résumé de Bilan ---
        let mut puce_clic = None;
        if self.vide() {
            let mut x = plein.left() + 14.0;
            for (t, lib, ic) in self.puces() {
                let g = ui.painter().layout_no_wrap(lib.to_string(), FontId::proportional(13.0), p.text);
                let k = ui.painter().layout_no_wrap(t.to_string(), FontId::monospace(12.0), p.muted);
                let w = g.size().x + k.size().x + 44.0;
                if x + w > plein.right() - 10.0 {
                    break;
                }
                let rc = Rect::from_min_size(pos2(x, y + 9.0), vec2(w, 28.0));
                let rep = ui.interact(rc, ui.scope_id().with(("puce", t)), Sense::click());
                ui.painter().rect_filled(rc, 14.0, if rep.hovered() { p.hover_bg } else { socle::theme::blend(p.surface, p.accent, 0.08) });
                socle::icons::paint(ui.painter(), Rect::from_center_size(pos2(rc.left() + 16.0, rc.center().y), vec2(16.0, 16.0)), ic, p.accent);
                let kx = rc.left() + 30.0;
                ui.painter().galley(pos2(kx, rc.center().y - k.size().y / 2.0), k.clone(), p.muted);
                ui.painter().galley(pos2(kx + k.size().x + 6.0, rc.center().y - g.size().y / 2.0), g, p.text);
                if rep.clicked() {
                    puce_clic = Some(t);
                }
                x += w + 8.0;
            }
            y += PUCES;
            if let Some(b) = self.bilan_ligne() {
                let rc = Rect::from_min_size(pos2(plein.left(), y), vec2(plein.width(), 26.0));
                socle::icons::paint(ui.painter(), Rect::from_center_size(pos2(rc.left() + 24.0, rc.center().y - 3.0), vec2(16.0, 16.0)), socle::icons::Icon::Chart, p.muted);
                socle::ui::paint_text_left(ui, Rect::from_min_size(rc.min - vec2(0.0, 3.0), rc.size()), rc.left() + 42.0, b, FontId::proportional(12.5), p.muted, rc.width() - 60.0);
                y += 26.0;
            }
        }

        // --- Résultats par section ---
        let lignes = self.lignes();
        let total: f32 = lignes.iter().map(|x| x.1).sum();
        let mut clic = None;
        if total > 0.0 {
            let haut_liste = total.min(LISTE_MAX);
            // Décalage pour garder la ligne active visible.
            let (mut debut_sel, mut acc) = (0.0, 0.0);
            for (l, h) in &lignes {
                if let Ligne::Res(i) = l
                    && *i == self.i
                {
                    debut_sel = acc;
                    acc += h;
                    break;
                }
                acc += h;
            }
            let decalage = (acc - haut_liste).max(0.0).min(debut_sel);
            let zone = Rect::from_min_size(pos2(plein.left(), y + 4.0), vec2(plein.width(), haut_liste));
            let painter = ui.painter().with_clip_rect(zone);
            let mut yy = zone.top() - decalage;
            let q_hl = self.q.clone();
            for (l, h) in &lignes {
                let row = Rect::from_min_size(pos2(plein.left() + 8.0, yy), vec2(plein.width() - 16.0, *h));
                yy += h;
                if row.bottom() < zone.top() || row.top() > zone.bottom() {
                    continue;
                }
                match l {
                    Ligne::Section(s) => {
                        let g = painter.layout_no_wrap(s.to_uppercase(), FontId::proportional(11.0), p.muted);
                        painter.galley(pos2(row.left() + 12.0, row.bottom() - g.size().y - 5.0), g, p.muted);
                    }
                    Ligne::Res(k) => {
                        let k = *k;
                        let r = &self.res[k];
                        let rep = ui.interact(row.intersect(zone), ui.scope_id().with(("ligne", k)), Sense::click());
                        if rep.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            self.i = k;
                        }
                        if rep.clicked() {
                            clic = Some((k, ui.input(|i| i.modifiers.shift)));
                        }
                        let actif = k == self.i;
                        if actif {
                            painter.rect_filled(row, 8.0, p.sel_bg);
                            painter.rect_filled(Rect::from_min_size(pos2(row.left(), row.top() + 10.0), vec2(3.0, row.height() - 20.0)), 2.0, p.accent);
                        }
                        self.peindre_icone(ui, r, pos2(row.left() + 26.0, row.center().y), actif);
                        let x = row.left() + 52.0;
                        let droite = if actif { 150.0 } else { 16.0 };
                        let max_w = row.right() - x - droite;
                        let calc = r.ico == Ico::Calc && *h == LIGNE_CALC;
                        let taille = if calc { 26.0 } else { 15.0 };
                        let g = socle::ui::highlighted(ui, &r.titre, if calc { "" } else { &q_hl }, FontId::proportional(taille), p.text, p.accent, max_w);
                        let ty = if r.sous.is_empty() { row.center().y - g.size().y / 2.0 } else { row.top() + if calc { 6.0 } else { 6.0 } };
                        let gy = g.size().y;
                        painter.galley(pos2(x, ty), g, p.text);
                        if !r.sous.is_empty() {
                            let g = socle::ui::trunc(ui, &r.sous, FontId::proportional(12.0), p.muted, max_w);
                            painter.galley(pos2(x, ty + gy + 1.0), g, p.muted);
                        }
                        if actif {
                            // Touches à droite : ↵ (ou « confirmer »), ⇧↵ si l'action secondaire existe.
                            let confirmer = self.confirm == Some(k);
                            let mut xr = row.right() - 12.0;
                            let mut touche = |txt: &str, lib: &str, c: Color32| {
                                let gl = painter.layout_no_wrap(lib.to_string(), FontId::proportional(11.5), c);
                                xr -= gl.size().x;
                                painter.galley(pos2(xr, row.center().y - gl.size().y / 2.0), gl, c);
                                let gt = painter.layout_no_wrap(txt.to_string(), FontId::proportional(12.0), c);
                                let kc = Rect::from_min_size(pos2(xr - gt.size().x - 14.0, row.center().y - 10.0), vec2(gt.size().x + 10.0, 20.0));
                                painter.rect_stroke(kc, 5.0, egui::Stroke::new(1.0, c), egui::StrokeKind::Inside);
                                painter.galley(pos2(kc.left() + 5.0, kc.center().y - gt.size().y / 2.0), gt, c);
                                xr = kc.left() - 10.0;
                            };
                            if confirmer {
                                touche("↵", " confirmer", p.del);
                            } else {
                                touche("↵", "", p.accent);
                                if r.alt.is_some() {
                                    touche("Maj ↵", " dossier", p.muted);
                                }
                            }
                        }
                    }
                }
            }
            y += haut_liste + 8.0;
        }

        // --- Pied : message ou raccourcis ---
        let pied = Rect::from_min_size(pos2(plein.left(), plein.bottom() - PIED), vec2(plein.width(), PIED));
        let _ = y;
        ui.painter().hline(plein.x_range(), pied.top(), egui::Stroke::new(1.0, socle::theme::blend(p.surface, p.border, 0.6)));
        match &self.msg {
            Some((m, err)) => socle::ui::paint_text_left(ui, pied, pied.left() + 16.0, m, FontId::proportional(12.0), if *err { p.del } else { p.accent }, pied.width() - 32.0),
            None => {
                let aide = if self.prefixe.is_some() { "↵ ouvrir   Maj+↵ dossier   Tab compléter   ⌫ retirer le préfixe   Échap fermer" } else { "↵ ouvrir   Maj+↵ dossier   Tab compléter   ↑↓ choisir   Échap fermer" };
                socle::ui::paint_text_left(ui, pied, pied.left() + 16.0, aide, FontId::proportional(11.5), p.muted, pied.width() - 32.0);
            }
        }
        socle::ui::paint_text_right(ui, pied, pied.right() - 16.0, "Lanceur", FontId::proportional(11.0), socle::theme::blend(p.muted, p.surface, 0.4));

        if let Some(t) = puce_clic {
            if t == ";" {
                self.q = ";".into();
            } else {
                self.prefixe = PREFIXES.iter().find(|x| x.0 == t).map(|x| x.0);
            }
            self.focus = true;
            self.calculer();
        }
        if let Some((k, alt)) = clic {
            self.i = k;
            self.choisir(k, alt);
        }
    }

    pub fn erreur(&mut self, e: String) {
        self.msg = Some((e, true));
    }

    /// Mode test : tape une requête comme l'utilisateur.
    pub fn taper(&mut self, q: &str) {
        self.q = q.to_string();
        self.prefixe = None;
        self.normaliser();
        self.calculer();
    }

    /// `--action` reçu par l'instance ouverte : exécuté en fond, sans montrer la barre.
    pub fn action_en_fond(&mut self, q: String) {
        let d = self.d.clone();
        let mut s = Settings { demarrage: self.s.demarrage, appris: self.s.appris.clone(), snippets: self.s.snippets.clone(), derniere: self.s.derniere.clone(), last_update_check: 0 };
        self.fond(move || Evt::Fait(App::action_directe(&d, &mut s, &q)));
    }
}

fn icone(i: Ico) -> socle::icons::Icon {
    use socle::icons::Icon;
    match i {
        Ico::App => Icon::Grid,
        Ico::Maison => Icon::Logo,
        Ico::Jeu => Icon::Gamepad,
        Ico::Dossier => Icon::Folder,
        Ico::Fichier => Icon::File,
        Ico::Calc => Icon::Command,
        Ico::Web => Icon::Link,
        Ico::Commande => Icon::Settings,
        Ico::Code => Icon::Code,
        Ico::Terminal | Ico::Claude => Icon::Terminal,
        Ico::Git => Icon::Git,
        Ico::Copier => Icon::Copy,
        Ico::Tuer => Icon::Close,
        Ico::Son => Icon::Volume,
        Ico::Memoire => Icon::History,
        Ico::Tache => Icon::Check,
        Ico::Jauge => Icon::Memory,
        Ico::Video => Icon::Play,
        Ico::Repeter => Icon::Refresh,
        Ico::Texte => Icon::TextFile,
    }
}
