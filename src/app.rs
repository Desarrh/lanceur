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
const CHAMP: f32 = 58.0;
const LIGNE: f32 = 46.0;
const MAX_LIGNES: usize = 9;

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
}

impl App {
    pub fn new(ctx: &egui::Context, s: Settings, d: Donnees) -> App {
        socle::theme::install_fonts(ctx);
        let pal = socle::theme::palette(socle::theme::system_dark(ctx));
        socle::theme::apply(ctx, &pal);
        let (tx, rx) = channel();
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

    pub fn hauteur(&self) -> f32 {
        let n = self.res.len().min(MAX_LIGNES) as f32;
        CHAMP + if n > 0.0 { n * LIGNE + 8.0 } else { 0.0 } + if self.msg.is_some() { 28.0 } else { 0.0 }
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
            moteur::requete(&self.q, &cx)
        };
        if self.q.trim().is_empty() && let Some(m) = &self.maj {
            res.insert(0, Res::new(Ico::Repeter, format!("Mettre à jour le Lanceur ({})", m.version), "Entrée : installe et relance", Action::Rien).cle("maj"));
        }
        if self.q_calc.as_deref() != Some(self.q.as_str()) {
            self.i = 0;
            self.confirm = None;
        }
        if std::env::var_os("LANCEUR_MESURE").is_some() {
            eprintln!("recherche « {} » : {:.2} ms", self.q, t0.elapsed().as_secs_f64() * 1000.0);
        }
        self.q_calc = Some(self.q.clone());
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
                Evt::Async(r, v) => {
                    self.en_cours.remove(&r);
                    if matches!(r, Req::Fichiers(_) | Req::Memoire(_)) {
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
            self.s.appris.noter(&self.q, c);
        }
        let mut derniere = r.clone();
        derniere.action = action.clone();
        derniere.alt = None;
        self.s.derniere = Some(derniere);
        self.saver.touch();
        let garder = matches!(action, Action::Sortie { .. });
        if garder {
            self.msg = Some(("…".into(), false));
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
        let (bas, haut, entree, maj_entree, echap, tab) = self.ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::SHIFT, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Tab),
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
        if echap {
            if self.q.is_empty() {
                self.cacher = true;
            } else {
                self.q.clear();
                self.calculer();
            }
        }
        if tab && let Some(r) = self.res.get(self.i) {
            // Complète avec le titre (utile pour « d proj », « p jeu »).
            if let Some((tete, _)) = self.q.split_once(' ') {
                let nom = r.titre.split(" : ").next().unwrap_or(&r.titre).to_string();
                self.q = format!("{tete} {nom} ");
            } else {
                self.q = r.titre.clone();
            }
            self.calculer();
        }
        if entree {
            self.choisir(self.i, false);
        }
        if maj_entree {
            self.choisir(self.i, true);
        }
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        let p = self.pal.clone();
        let plein = ui.max_rect();
        ui.painter().rect_filled(plein, 10.0, p.surface);
        ui.painter().rect_stroke(plein.shrink(0.5), 10.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        // Champ de saisie.
        let champ = Rect::from_min_size(plein.min, vec2(plein.width(), CHAMP));
        socle::icons::paint(ui.painter(), Rect::from_center_size(pos2(champ.left() + 30.0, champ.center().y), vec2(22.0, 22.0)), socle::icons::Icon::Search, p.muted);
        let edit_rect = Rect::from_min_max(pos2(champ.left() + 52.0, champ.top() + 12.0), pos2(champ.right() - 16.0, champ.bottom() - 12.0));
        let avant = self.q.clone();
        let resp = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut self.q)
                .id_salt("q")
                .frame(egui::Frame::NONE)
                .font(FontId::proportional(20.0))
                .text_color(p.text)
                .hint_text(egui::RichText::new(if self.sel.is_some() { "Texte sélectionné : choisis une action, ou tape…" } else { "Apps, jeux, fichiers, calcul… (d, p, k, y, v, c, ;, :port, !)" }).color(p.muted))
                .desired_width(edit_rect.width()),
        );
        if self.focus {
            resp.request_focus();
            self.focus = false;
        }
        if self.q != avant {
            self.calculer();
        }
        if self.res.is_empty() && self.msg.is_none() {
            return;
        }
        ui.painter().hline(plein.x_range(), champ.bottom(), egui::Stroke::new(1.0, p.border));
        // Résultats.
        let mut y = champ.bottom() + 4.0;
        let debut = self.i.saturating_sub(MAX_LIGNES - 1);
        let mut clic = None;
        for (k, r) in self.res.iter().enumerate().skip(debut).take(MAX_LIGNES) {
            let row = Rect::from_min_size(pos2(plein.left() + 6.0, y), vec2(plein.width() - 12.0, LIGNE));
            let rep = ui.interact(row, ui.scope_id().with(("ligne", k)), Sense::click());
            if rep.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                self.i = k;
            }
            if rep.clicked() {
                clic = Some((k, ui.input(|i| i.modifiers.shift)));
            }
            let actif = k == self.i;
            if actif {
                ui.painter().rect_filled(row, 6.0, p.sel_bg);
            }
            let ir = Rect::from_center_size(pos2(row.left() + 22.0, row.center().y), vec2(20.0, 20.0));
            if let Some([r_, g, b]) = r.couleur {
                ui.painter().rect_filled(ir.expand(2.0), 5.0, Color32::from_rgb(r_, g, b));
                ui.painter().rect_stroke(ir.expand(2.0), 5.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Outside);
            } else {
                socle::icons::paint(ui.painter(), ir, icone(r.ico), if actif { p.accent } else { p.muted });
            }
            let x = row.left() + 46.0;
            let droite = if actif { 120.0 } else { 16.0 };
            let max_w = row.right() - x - droite;
            let q_hl = self.q.split_once(' ').map_or(self.q.as_str(), |(t, rest)| if t.len() <= 2 { rest } else { self.q.as_str() });
            let g = socle::ui::highlighted(ui, &r.titre, q_hl, FontId::proportional(15.0), p.text, p.accent, max_w);
            let ty = if r.sous.is_empty() { row.center().y - g.size().y / 2.0 } else { row.top() + 5.0 };
            ui.painter().galley(pos2(x, ty), g, p.text);
            if !r.sous.is_empty() {
                let g = socle::ui::trunc(ui, &r.sous, FontId::proportional(12.0), p.muted, max_w);
                ui.painter().galley(pos2(x, row.top() + 25.0), g, p.muted);
            }
            if actif {
                let txt = if self.confirm == Some(k) {
                    "Entrée : confirmer"
                } else if r.alt.is_some() {
                    "↵ · Maj+↵ dossier"
                } else {
                    "↵"
                };
                socle::ui::paint_text_right(ui, row, row.right() - 12.0, txt, FontId::proportional(11.5), if self.confirm == Some(k) { p.del } else { p.muted });
            }
            y += LIGNE;
        }
        if let Some((m, err)) = &self.msg {
            let r = Rect::from_min_size(pos2(plein.left(), plein.bottom() - 28.0), vec2(plein.width(), 28.0));
            socle::ui::paint_text_left(ui, r, r.left() + 16.0, m, FontId::proportional(12.0), if *err { p.del } else { p.muted }, r.width() - 32.0);
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
        Ico::Jauge => Icon::Memory,
        Ico::Video => Icon::Play,
        Ico::Repeter => Icon::Refresh,
        Ico::Texte => Icon::TextFile,
    }
}
