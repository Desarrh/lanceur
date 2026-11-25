#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod calc;
mod fond;
mod icones;
mod integr;
mod moteur;
mod sources;
mod sysw;
mod uia;

use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId, WindowLevel};

enum Evt {
    Repaint(Instant),
    Fond(fond::Fond),
    /// Message d'un 2e lancement (instance unique).
    Instance(Vec<String>),
}

/// Rendu CPU (comme l'explorateur) : pas de contexte GPU.
struct Gfx {
    window: Rc<Window>,
    egui: egui_winit::State,
    _context: softbuffer::Context<Rc<Window>>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    renderer: EguiSoftwareRender,
}

struct Runner {
    ctx: egui::Context,
    gfx: Option<Gfx>,
    app: Option<app::App>,
    init: Option<(app::Settings, app::Donnees)>,
    next_repaint: Option<Instant>,
    visible: bool,
    /// Heure d'ouverture : mesure (LANCEUR_MESURE) et garde contre une perte de focus immédiate.
    ouvert_a: Option<Instant>,
    mesure: bool,
    hauteur: f32,
    instance: Option<socle::instance::Instance>,
    proxy: winit::event_loop::EventLoopProxy<Evt>,
    /// Mode test : requête à taper puis capture (LANCEUR_REQUETE, LANCEUR_CAPTURE).
    images: u32,
    /// Apparition en cours : début et position finale.
    anim: Option<(Instant, PhysicalPosition<i32>)>,
    /// Raccourci global refusé : affiché à la 1re ouverture.
    erreur: Option<String>,
}

fn hwnd(w: &Window) -> isize {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match w.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

/// Coins arrondis de Windows 11 et liseré de la couleur d'accent (DWM : gratuit, rien à peindre).
fn dwm(h: isize, accent: egui::Color32) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    let coin = DWMWCP_ROUND;
    let couleur: u32 = accent.r() as u32 | (accent.g() as u32) << 8 | (accent.b() as u32) << 16;
    unsafe {
        DwmSetWindowAttribute(h as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &coin as *const _ as _, 4);
        DwmSetWindowAttribute(h as _, DWMWA_BORDER_COLOR as u32, &couleur as *const _ as _, 4);
    }
}

/// Opacité de la fenêtre (fenêtre « layered » pendant l'animation, normale ensuite).
fn opacite(h: isize, a: Option<u8>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW, WS_EX_LAYERED};
    unsafe {
        let ex = GetWindowLongPtrW(h as _, GWL_EXSTYLE);
        match a {
            Some(a) => {
                SetWindowLongPtrW(h as _, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
                SetLayeredWindowAttributes(h as _, 0, a, LWA_ALPHA);
            }
            None => {
                SetWindowLongPtrW(h as _, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
            }
        }
    }
}

/// Durée de l'apparition (fondu + glissement de 10 px vers le haut).
const ANIM_MS: f32 = 110.0;

/// Écran sous la souris (l'utilisateur regarde là), barre au quart supérieur.
fn position(w: &Window, hauteur_px: f64) -> Option<PhysicalPosition<i32>> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    let mons: Vec<_> = w.available_monitors().collect();
    let m = mons
        .iter()
        .find(|m| {
            let (p, s) = (m.position(), m.size());
            pt.x >= p.x && pt.y >= p.y && pt.x < p.x + s.width as i32 && pt.y < p.y + s.height as i32
        })
        .cloned()
        .or_else(|| w.primary_monitor())?;
    let (p, s) = (m.position(), m.size());
    let larg = app::LARGEUR as f64 * m.scale_factor();
    let _ = hauteur_px;
    Some(PhysicalPosition::new(p.x + ((s.width as f64 - larg) / 2.0) as i32, p.y + (s.height as f64 * 0.22) as i32))
}

impl Runner {
    fn afficher(&mut self, sel: Option<String>) {
        let (Some(gfx), Some(app)) = (self.gfx.as_ref(), self.app.as_mut()) else { return };
        if self.visible {
            if sel.is_some() {
                app.set_selection(sel.unwrap_or_default());
            }
            gfx.window.focus_window();
            return;
        }
        self.ouvert_a = Some(Instant::now());
        app.ouvrir(sel);
        let h = app.hauteur();
        self.hauteur = h;
        let _ = gfx.window.request_inner_size(LogicalSize::new(app::LARGEUR, h));
        let pos = position(&gfx.window, h as f64 * gfx.window.scale_factor());
        if let Some(p) = pos {
            gfx.window.set_outer_position(PhysicalPosition::new(p.x, p.y + 10));
        }
        if socle::testmode::enabled() {
            socle::testmode::hide(hwnd(&gfx.window)); // opacité 0, sans activation, avant d'apparaître
            gfx.window.set_visible(true);
            if let Some(p) = pos {
                gfx.window.set_outer_position(p);
            }
        } else {
            // La saisie marche dès la 1re image : l'animation ne fait que l'habiller.
            opacite(hwnd(&gfx.window), Some(0));
            gfx.window.set_visible(true);
            gfx.window.focus_window();
            self.anim = pos.map(|p| (Instant::now(), p));
            if pos.is_none() {
                opacite(hwnd(&gfx.window), None);
            }
        }
        self.visible = true;
        gfx.window.request_redraw();
    }

    fn cacher(&mut self) {
        if let Some(g) = &self.gfx {
            g.window.set_visible(false);
            if self.anim.take().is_some() {
                opacite(hwnd(&g.window), None);
            }
        }
        self.visible = false;
        if let Some(app) = self.app.as_mut() {
            app.cacher = false;
            app.flush();
        }
    }

    fn paint(&mut self, el: &ActiveEventLoop) {
        let (Some(gfx), Some(app)) = (self.gfx.as_mut(), self.app.as_mut()) else { return };
        let size = gfx.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        let mut raw = gfx.egui.take_egui_input(&gfx.window);
        socle::testmode::prepare_input(&mut raw);
        let out = self.ctx.run_ui(raw, |ui| {
            app.tick();
            if self.visible {
                app.handle_keys();
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| app.draw(ui));
            }
        });
        gfx.egui.handle_platform_output(&gfx.window, out.platform_output);
        if self.visible {
            let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
            if gfx.surface.resize(w, h).is_ok() && let Ok(mut buf) = gfx.surface.buffer_mut() {
                buf.fill(app.pal.clear_u32());
                {
                    let px: &mut [[u8; 4]] = bytemuck::cast_slice_mut(&mut buf[..]);
                    let mut target = BufferMutRef::new(px, w.get() as usize, h.get() as usize);
                    gfx.renderer.render(&mut target, &prims, &out.textures_delta, out.pixels_per_point);
                }
                let _ = buf.present();
            }
            if let Some((t0, p)) = self.anim {
                let t = (t0.elapsed().as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
                let e = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t); // ease-out
                let h = hwnd(&gfx.window);
                if t >= 1.0 {
                    gfx.window.set_outer_position(p);
                    opacite(h, None);
                    self.anim = None;
                } else {
                    gfx.window.set_outer_position(PhysicalPosition::new(p.x, p.y + ((1.0 - e) * 10.0) as i32));
                    opacite(h, Some((e * 255.0) as u8));
                    gfx.window.request_redraw();
                }
            }
            if self.mesure && let Some(t) = self.ouvert_a.take() {
                eprintln!("ouverture : {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        // La hauteur suit le nombre de résultats.
        let hh = app.hauteur();
        if self.visible && (hh - self.hauteur).abs() > 0.5 {
            self.hauteur = hh;
            let _ = gfx.window.request_inner_size(LogicalSize::new(app::LARGEUR, hh));
            gfx.window.request_redraw();
        }
        // Mode test : `LANCEUR_CAPTURE=f.png` capture la barre invisible puis quitte.
        if socle::testmode::enabled() && self.visible {
            self.images += 1;
            let attente = std::env::var("LANCEUR_ATTENTE").ok().and_then(|x| x.parse().ok()).unwrap_or(0u64);
            let pret = T0.get().is_some_and(|t| t.elapsed().as_millis() as u64 >= attente);
            if let (Some(f), true) = (std::env::var_os("LANCEUR_CAPTURE"), self.images > 6 && pret) {
                if let Some((w, h, px)) = socle::testmode::capture(hwnd(&gfx.window)) {
                    let _ = image::save_buffer(std::path::Path::new(&f), &px, w, h, image::ColorType::Rgba8);
                }
                app.quitter = true;
            } else {
                gfx.window.request_redraw();
            }
        }
        if app.quitter {
            app.flush();
            el.exit();
            return;
        }
        if app.cacher {
            self.cacher();
        }
    }
}

impl ApplicationHandler<Evt> for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("Lanceur")
            .with_inner_size(LogicalSize::new(app::LARGEUR, 58.0))
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true)
            .with_undecorated_shadow(true)
            .with_window_icon(window_icon());
        // Règle n° 0 : en test, jamais de fenêtre active (ShowWindow ne doit pas voler le focus).
        if socle::testmode::enabled() {
            attrs = attrs.with_active(false);
        }
        let Ok(window) = el.create_window(attrs) else {
            el.exit();
            return;
        };
        let window = Rc::new(window);
        fond::BARRE.store(hwnd(&window), std::sync::atomic::Ordering::Relaxed);
        let (Ok(context), egui) = (
            softbuffer::Context::new(window.clone()),
            egui_winit::State::new(self.ctx.clone(), egui::ViewportId::ROOT, &*window, Some(window.scale_factor() as f32), None, Some(8192)),
        ) else {
            el.exit();
            return;
        };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else {
            el.exit();
            return;
        };
        self.gfx = Some(Gfx { window, egui, _context: context, surface, renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra) });
        if let Some((s, d)) = self.init.take() {
            let mut a = app::App::new(&self.ctx, s, d);
            if let Some(e) = self.erreur.take() {
                a.erreur(e);
            }
            if let Some(g) = &self.gfx {
                dwm(hwnd(&g.window), a.pal.accent);
            }
            self.app = Some(a);
        }
        if let Some(inst) = self.instance.take() {
            let p = self.proxy.clone();
            inst.listen(move |args| {
                let _ = p.send_event(Evt::Instance(args));
            });
        }
        // Mode test : ouvre la barre invisible avec une requête, pour la capture.
        if socle::testmode::enabled() {
            self.afficher(None);
            if let (Ok(q), Some(app)) = (std::env::var("LANCEUR_REQUETE"), self.app.as_mut()) {
                app.taper(&q);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let resp = gfx.egui.on_window_event(&gfx.window, &event);
        match event {
            WindowEvent::CloseRequested => self.cacher(),
            WindowEvent::Focused(false) if self.visible && !socle::testmode::enabled() => self.cacher(),
            WindowEvent::RedrawRequested => self.paint(el),
            _ if resp.repaint => gfx.window.request_redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, e: Evt) {
        match e {
            Evt::Repaint(t) => {
                if t <= Instant::now() {
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                } else {
                    self.next_repaint = Some(self.next_repaint.map_or(t, |n| n.min(t)));
                }
            }
            Evt::Fond(fond::Fond::Afficher { sel, .. }) => {
                if self.visible && sel.is_none() {
                    self.cacher(); // Alt+Espace une 2e fois : referme
                } else {
                    self.afficher(sel);
                }
            }
            Evt::Fond(fond::Fond::Selection(s)) => {
                if let Some(app) = self.app.as_mut().filter(|_| self.visible) {
                    app.set_selection(s);
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                }
            }
            Evt::Fond(fond::Fond::Quitter) => {
                if let Some(app) = self.app.as_mut() {
                    app.flush();
                }
                el.exit();
            }
            Evt::Instance(args) => {
                if let Some(i) = args.iter().position(|a| a == "--action") {
                    let q = args.get(i + 1).cloned().unwrap_or_default();
                    if let Some(app) = self.app.as_mut() {
                        app.action_en_fond(q);
                    }
                } else if !args.iter().any(|a| a == "--fond") {
                    self.afficher(None);
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.next_repaint {
            Some(t) if t <= Instant::now() => {
                self.next_repaint = None;
                if let Some(g) = &self.gfx {
                    g.window.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(app) = self.app.as_mut() {
            app.flush();
        }
    }
}

fn window_icon() -> Option<winit::window::Icon> {
    let img = image::load_from_memory(include_bytes!("../assets/icon64.png")).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn main() {
    T0.get_or_init(Instant::now);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if socle::testmode::enabled() {
        socle::testmode::low_priority();
    }
    socle::crash::install(app::APP, env!("CARGO_PKG_VERSION"));

    // `Lanceur.exe --action "texte"` : exécute sans interface (Stream Deck). Transmis à l'instance
    // ouverte si elle existe (son index est chaud), sinon fait ici puis quitte.
    if let Some(i) = args.iter().position(|a| a == "--action") {
        let q = args.get(i + 1).cloned().unwrap_or_default();
        if !socle::testmode::enabled() && let socle::instance::Start::Forwarded = socle::instance::start(app::APP, &["--action".into(), q.clone()]) {
            return;
        }
        let mut s = app::Settings::load();
        let d = app::charger(&s.appris, false);
        let r = app::App::action_directe(&d, &mut s, &q);
        let _ = socle::settings::save(app::APP, &s);
        match r {
            Ok(m) => {
                if let Some(m) = m {
                    println!("{m}");
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
        return;
    }

    socle::update::rollback_on_panic();
    if socle::update::startup() == socle::update::Startup::RolledBack {
        return;
    }
    socle::update::cleanup_old();
    let after_update = socle::update::is_after_update();
    // Déjà lancé : le 2e lancement ouvre la barre de l'instance existante.
    let mut instance = None;
    // Règle n° 0 : une instance de test ne parle jamais à la vraie (elle lui ferait ouvrir la barre).
    let start = if socle::testmode::enabled() { socle::instance::Start::Alone } else { socle::instance::start(app::APP, &args) };
    match start {
        socle::instance::Start::Forwarded if !after_update => return,
        socle::instance::Start::Primary(i) => instance = Some(i),
        _ => {}
    }
    let settings = app::Settings::load();
    if !socle::testmode::enabled() {
        let _ = fond::autostart_set(settings.demarrage);
    }
    let donnees = app::charger(&settings.appris, false);

    let Ok(event_loop) = EventLoop::<Evt>::with_user_event().build() else { return };
    let proxy = event_loop.create_proxy();
    let ctx = egui::Context::default();
    let p = proxy.clone();
    ctx.set_request_repaint_callback(move |info| {
        let _ = p.send_event(Evt::Repaint(Instant::now() + info.delay));
    });
    let mut erreur = None;
    // Règle n° 0 : en mode test, ni raccourci global ni icône de notification.
    if !socle::testmode::enabled() {
        let p = std::sync::Mutex::new(proxy.clone());
        let rx = fond::start(move |f| {
            if let Ok(p) = p.lock() {
                let _ = p.send_event(Evt::Fond(f));
            }
        });
        if let Ok(Err(e)) = rx.recv_timeout(std::time::Duration::from_secs(2)) {
            erreur = Some(e);
        }
    }
    let mut runner = Runner {
        ctx,
        gfx: None,
        app: None,
        init: Some((settings, donnees)),
        next_repaint: None,
        visible: false,
        ouvert_a: None,
        mesure: std::env::var_os("LANCEUR_MESURE").is_some(),
        hauteur: 0.0,
        instance,
        proxy,
        images: 0,
        anim: None,
        erreur,
    };
    let _ = event_loop.run_app(&mut runner);
}
