#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bundle;
mod game;
mod i18n;
mod permanent;
mod settings;

use eframe::egui;
use game::{CameraState, GameError, GameSession, MAX_LIMIT};
use i18n::Catalog;
use permanent::InstalledMod;
use settings::Settings;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum PermanentAction {
    Install,
    Reset,
}

struct Toolkit {
    session: Option<GameSession>,
    connecting: Option<Receiver<Result<GameSession, GameError>>>,
    modifying: Option<(PermanentAction, Receiver<Result<(), GameError>>)>,
    installed: Option<InstalledMod>,
    installation_unknown: bool,
    game_dir: Option<PathBuf>,
    camera: Option<CameraState>,
    catalog: Catalog,
    settings: Settings,
    limit: f32,
    error: Option<String>,
    settings_error: Option<String>,
    notice: &'static str,
    last_poll: Instant,
    next_connect: Instant,
    initialize_camera: bool,
    waiting_for_camera: bool,
    close_failed: bool,
    force_close: bool,
}

impl Toolkit {
    fn new(ctx: &egui::Context) -> Self {
        ctx.set_visuals(egui::Visuals::dark());
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(12.0, 8.0);
            style.spacing.interact_size.y = 32.0;
            style.visuals.panel_fill = egui::Color32::from_rgb(21, 26, 34);
            style.visuals.override_text_color = Some(egui::Color32::from_rgb(230, 236, 245));
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(40, 95, 134);
            style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        });
        let catalog = Catalog::system();
        let (settings, settings_error) = match Settings::load() {
            Ok(settings) => (settings, None),
            Err(detail) => (
                Settings::default(),
                Some(catalog.format("settings_error", &[("detail", detail)])),
            ),
        };
        let (installed, installation_unknown, error) = match permanent::installed() {
            Ok(installed) => (installed, false, None),
            Err(error) => (None, true, Some(catalog.error(&error))),
        };
        let notice = if installed.is_some() {
            "permanent_installed"
        } else if installation_unknown {
            "permanent_unknown"
        } else if settings.zoom_limit.is_some() {
            "saved_pending"
        } else {
            "no_saved_limit"
        };
        Self {
            session: None,
            connecting: None,
            modifying: None,
            game_dir: installed
                .as_ref()
                .map(|installed| installed.game_dir.clone()),
            limit: installed
                .as_ref()
                .map(|installed| installed.limit)
                .or(settings.zoom_limit)
                .unwrap_or(300.0),
            installed,
            installation_unknown,
            camera: None,
            catalog,
            settings,
            error,
            settings_error,
            notice,
            last_poll: Instant::now(),
            next_connect: Instant::now(),
            initialize_camera: false,
            waiting_for_camera: false,
            close_failed: false,
            force_close: false,
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.connecting = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(GameSession::connect());
            ctx.request_repaint();
        });
    }

    fn save_settings(&mut self, zoom_limit: Option<f32>) -> bool {
        let candidate = Settings { zoom_limit };
        match candidate.save() {
            Ok(()) => {
                self.settings = candidate;
                self.settings_error = None;
                true
            }
            Err(detail) => {
                self.settings_error =
                    Some(self.catalog.format("settings_error", &[("detail", detail)]));
                false
            }
        }
    }

    fn modify_permanent(&mut self, action: PermanentAction, ctx: &egui::Context) {
        if matches!(action, PermanentAction::Reset)
            && self.installed.is_none()
            && !self.installation_unknown
        {
            self.reset();
            return;
        }
        let game_dir = self.game_dir.clone();
        let limit = self.limit;
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.modifying = Some((action, receiver));
        self.error = None;
        std::thread::spawn(move || {
            let result = match action {
                PermanentAction::Install => game_dir
                    .as_deref()
                    .ok_or_else(|| GameError::from("permanent_find_game"))
                    .and_then(|game_dir| permanent::install(game_dir, limit)),
                PermanentAction::Reset => permanent::reset(),
            };
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn poll_modification(&mut self) {
        let result =
            self.modifying
                .as_ref()
                .and_then(|(action, receiver)| match receiver.try_recv() {
                    Ok(result) => Some((*action, result)),
                    Err(TryRecvError::Disconnected) => {
                        Some((*action, Err("permanent_worker_failed".into())))
                    }
                    Err(TryRecvError::Empty) => None,
                });
        let Some((action, result)) = result else {
            return;
        };
        self.modifying = None;
        match result {
            Ok(()) => {
                self.session = None;
                self.camera = None;
                self.close_failed = false;
                self.error = None;
                if matches!(action, PermanentAction::Reset) {
                    let removed = self.save_settings(None);
                    self.limit = 300.0;
                    self.notice = if removed {
                        "permanent_removed"
                    } else {
                        "reset_settings_failed"
                    };
                } else {
                    self.notice = "permanent_installed";
                }
            }
            Err(error) => self.error = Some(self.catalog.error(&error)),
        }
        match permanent::installed() {
            Ok(installed) => {
                if let Some(installed) = &installed {
                    self.game_dir = Some(installed.game_dir.clone());
                    self.limit = installed.limit;
                }
                self.installed = installed;
                self.installation_unknown = false;
            }
            Err(error) => {
                self.installation_unknown = true;
                self.error = Some(self.catalog.error(&error));
            }
        }
        self.next_connect = Instant::now() + RETRY_INTERVAL;
    }

    fn apply(&mut self) {
        if let Some(session) = self.session.as_mut() {
            match session.set_limit(self.limit) {
                Ok(()) => {
                    self.error = None;
                    self.save_settings(Some(self.limit));
                    self.notice = "applied";
                }
                Err(error) => self.error = Some(self.catalog.error(&error)),
            }
        }
        self.read_camera();
    }

    fn reset(&mut self) {
        if let Some(session) = self.session.as_mut()
            && let Err(error) = session.restore()
        {
            self.error = Some(self.catalog.error(&error));
            return;
        }
        if !self.save_settings(None) {
            self.notice = "reset_settings_failed";
            self.read_camera();
            return;
        }
        self.error = None;
        self.close_failed = false;
        self.notice = if self.session.is_some() {
            "restored"
        } else {
            "no_saved_limit"
        };
        self.read_camera();
        if let Some(camera) = &self.camera {
            self.limit = camera.original;
        }
    }

    fn connection_lost(&mut self, error: Option<GameError>) {
        self.session = None;
        self.camera = None;
        self.initialize_camera = false;
        self.waiting_for_camera = error.as_ref().is_none_or(|error| {
            matches!(
                error.key,
                "error.game_loading" | "error.camera_unavailable" | "error.camera_changed"
            )
        });
        let transient = error.as_ref().is_none_or(|error| {
            matches!(
                error.key,
                "error.game_not_found"
                    | "error.game_loading"
                    | "error.camera_unavailable"
                    | "error.camera_changed"
                    | "error.game_access_lost"
            )
        });
        self.next_connect = Instant::now()
            + if transient {
                RETRY_INTERVAL
            } else {
                Duration::from_secs(10)
            };
        self.error = error
            .filter(|_| !transient)
            .map(|error| self.catalog.error(&error));
        self.notice = if self.settings.zoom_limit.is_some() {
            "saved_pending"
        } else {
            "no_saved_limit"
        };
    }

    fn read_camera(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.poll() {
            Ok(Some(camera)) => {
                let initialize = self.initialize_camera;
                self.initialize_camera = false;
                if initialize {
                    self.limit = self.settings.zoom_limit.unwrap_or(camera.maximum);
                    if camera.original > MAX_LIMIT {
                        self.error = Some(self.catalog.text("original_above_cap").to_owned());
                    } else if self.settings.zoom_limit.is_some() {
                        // Persisted limits are applied once for each newly acquired camera.
                        match session.set_limit(self.limit) {
                            Ok(()) => {
                                self.notice = "applied";
                                self.error = None;
                            }
                            Err(error) => self.error = Some(self.catalog.error(&error)),
                        }
                    }
                }
                self.camera = Some(camera);
                if initialize && let Some(session) = self.session.as_mut() {
                    match session.poll() {
                        Ok(camera) => self.camera = camera,
                        Err(error) => self.connection_lost(Some(error)),
                    }
                }
            }
            Ok(None) => self.connection_lost(None),
            Err(error) => self.connection_lost(Some(error)),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_modification();
        if self.modifying.is_some() || self.installed.is_some() || self.installation_unknown {
            return;
        }
        if let Some(receiver) = &self.connecting {
            match receiver.try_recv() {
                Ok(result) => {
                    self.connecting = None;
                    self.next_connect = Instant::now() + RETRY_INTERVAL;
                    match result {
                        Ok(session) => {
                            self.game_dir = Some(session.game_dir.clone());
                            self.close_failed = false;
                            self.session = Some(session);
                            self.initialize_camera = true;
                            self.error = None;
                            self.read_camera();
                        }
                        Err(error) => self.connection_lost(Some(error)),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.connecting = None;
                    self.next_connect = Instant::now() + RETRY_INTERVAL;
                    self.error = Some(self.catalog.text("connection_worker_failed").to_owned());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            self.read_camera();
        }
        if self.session.is_none()
            && self.connecting.is_none()
            && Instant::now() >= self.next_connect
        {
            self.connect(ctx);
        }
    }
}

impl eframe::App for Toolkit {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        ctx.request_repaint_after(POLL_INTERVAL);
        if self.modifying.is_some() && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        if ctx.input(|input| input.viewport().close_requested())
            && !self.force_close
            && !self.close_failed
            && let Some(session) = self.session.as_mut()
            && let Err(error) = session.restore()
            && !matches!(
                error.key,
                "error.game_access_lost" | "error.camera_changed" | "error.restore_camera_gone"
            )
        {
            self.close_failed = true;
            self.error = Some(self.catalog.format(
                "close_restore_failed",
                &[("detail", self.catalog.error(&error))],
            ));
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(24))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(egui::RichText::new("Subspace Toolkit").size(28.0).strong());
                    ui.label(
                        egui::RichText::new("Star Trek: Outposts Unknown")
                            .color(egui::Color32::from_rgb(183, 196, 213)),
                    );
                    ui.add_space(12.0);
                    let status = if self.modifying.is_some() {
                        "permanent_working"
                    } else if self.installation_unknown {
                        "permanent_unknown"
                    } else if self.installed.is_some() {
                        "permanent_installed"
                    } else if self.camera.is_some() {
                        "camera_connected"
                    } else if self.connecting.is_some() {
                        "connecting"
                    } else if self.session.is_some() || self.waiting_for_camera {
                        "waiting_camera"
                    } else {
                        "waiting_game"
                    };
                    ui.horizontal_wrapped(|ui| {
                        if self.modifying.is_some() {
                            ui.spinner();
                        }
                        ui.strong(self.catalog.text(status));
                    });
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.heading(self.catalog.text("zoom_limit"));
                    ui.label(self.catalog.text("zoom_description"));
                    let minimum = self.camera.as_ref().map_or(300.0, |camera| camera.original);
                    let idle = self.modifying.is_none();
                    let available = self.camera.is_some()
                        && minimum <= MAX_LIMIT
                        && idle
                        && self.installed.is_none()
                        && !self.installation_unknown;
                    let slider_available = self.game_dir.is_some()
                        && minimum <= MAX_LIMIT
                        && idle
                        && !self.installation_unknown;
                    ui.add_enabled_ui(slider_available, |ui| {
                        ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(120.0);
                        let slider = ui.add(
                            egui::Slider::new(&mut self.limit, minimum.min(MAX_LIMIT)..=MAX_LIMIT)
                                .step_by(1.0)
                                .fixed_decimals(0),
                        );
                        if available && slider.drag_stopped() {
                            self.apply();
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if self.installed.is_none()
                            && ui
                                .add_enabled(
                                    available,
                                    egui::Button::new(self.catalog.text("apply")),
                                )
                                .clicked()
                        {
                            self.apply();
                        }
                        if ui
                            .add_enabled(
                                slider_available && self.connecting.is_none(),
                                egui::Button::new(self.catalog.text("permanent_save")),
                            )
                            .clicked()
                        {
                            self.modify_permanent(PermanentAction::Install, ui.ctx());
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                idle && self.connecting.is_none()
                                    && (available
                                        || self.settings.zoom_limit.is_some()
                                        || self.settings_error.is_some()
                                        || self.installed.is_some()
                                        || self.installation_unknown),
                                egui::Button::new(self.catalog.text("permanent_reset")),
                            )
                            .clicked()
                        {
                            self.modify_permanent(PermanentAction::Reset, ui.ctx());
                        }
                    });
                    if let Some(camera) = &self.camera {
                        ui.add_space(8.0);
                        ui.label(self.catalog.format(
                            "readout",
                            &[
                                ("current", format!("{:.0}", camera.current)),
                                ("maximum", format!("{:.0}", camera.maximum)),
                                ("original", format!("{:.0}", camera.original)),
                            ],
                        ));
                    }
                    if let Some(installed) = &self.installed {
                        ui.label(self.catalog.format(
                            "permanent_limit",
                            &[("limit", format!("{:.0}", installed.limit))],
                        ));
                    }
                    ui.add_space(12.0);
                    if self.modifying.is_none() && self.notice != status {
                        ui.label(self.catalog.text(self.notice));
                    }
                    if self.installed.is_none()
                        && self.modifying.is_none()
                        && !self.installation_unknown
                    {
                        ui.label(self.catalog.text(if self.game_dir.is_some() {
                            "permanent_ready"
                        } else {
                            "permanent_find_game"
                        }));
                    }
                    for error in [&self.error, &self.settings_error].into_iter().flatten() {
                        ui.colored_label(egui::Color32::from_rgb(255, 170, 155), error);
                    }
                    if self.close_failed {
                        ui.label(self.catalog.text("close_warning"));
                        if ui.button(self.catalog.text("close_anyway")).clicked() {
                            self.force_close = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(self.catalog.text(if self.installed.is_some() {
                            "permanent_instructions"
                        } else {
                            "instructions"
                        }))
                        .small()
                        .color(egui::Color32::from_rgb(183, 196, 213)),
                    );
                });
            });
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Subspace Toolkit",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([590.0, 540.0])
                .with_min_inner_size([480.0, 360.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Toolkit::new(&cc.egui_ctx)))),
    )
}
