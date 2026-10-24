#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod i18n;
mod inject;
mod legacy;
mod permanent;
mod settings;

use eframe::egui;
use game::{CameraState, GameError, GameSession, MAX_LIMIT};
use i18n::Catalog;
use permanent::InstalledMod;
use settings::Settings;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum PermanentAction {
    Inspect,
    Install,
    Reset,
}

type InstallationResult = Result<Option<InstalledMod>, GameError>;

struct Toolkit {
    controller: Controller,
    show_mods: bool,
}

struct Controller {
    session: Option<GameSession>,
    connecting: Option<Receiver<Result<GameSession, GameError>>>,
    modifying: Option<(PermanentAction, Receiver<InstallationResult>)>,
    operation_progress: Arc<AtomicUsize>,
    installed: Option<InstalledMod>,
    installation_unknown: bool,
    game_dir: Option<PathBuf>,
    camera: Option<CameraState>,
    catalog: Catalog,
    settings: Settings,
    limit: f32,
    movement_speed: f32,
    save_after: Option<Instant>,
    error: Option<String>,
    settings_error: Option<String>,
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
        let catalog = Catalog::selected(None);
        let (settings, settings_error) = match Settings::load() {
            Ok(settings) => (settings, None),
            Err(detail) => (
                Settings::default(),
                Some(catalog.format("settings_error", &[("detail", detail)])),
            ),
        };
        let mut controller = Controller::new(settings, settings_error);
        controller.modify_permanent(PermanentAction::Inspect, ctx);
        Self {
            controller,
            show_mods: false,
        }
    }
}

impl Controller {
    fn new(settings: Settings, settings_error: Option<String>) -> Self {
        let catalog = Catalog::selected(settings.language.as_deref());
        Self {
            session: None,
            connecting: None,
            modifying: None,
            operation_progress: Arc::new(AtomicUsize::new(0)),
            game_dir: settings.game_dir.clone(),
            limit: settings.zoom_limit.unwrap_or(300.0),
            movement_speed: settings.movement_speed.unwrap_or(1.0),
            save_after: None,
            installed: None,
            installation_unknown: true,
            camera: None,
            catalog,
            settings,
            error: None,
            settings_error,
            last_poll: Instant::now(),
            next_connect: Instant::now(),
            initialize_camera: false,
            waiting_for_camera: false,
            close_failed: false,
            force_close: false,
        }
    }

    fn select_language(&mut self, language: Option<String>) {
        let candidate = Settings {
            language,
            ..self.settings.clone()
        };
        match candidate.save() {
            Ok(()) => {
                self.catalog = Catalog::selected(candidate.language.as_deref());
                self.settings = candidate;
                self.settings_error = None;
            }
            Err(detail) => {
                self.settings_error =
                    Some(self.catalog.format("settings_error", &[("detail", detail)]))
            }
        }
    }

    fn select_folder(&mut self, folder: PathBuf, ctx: &egui::Context) {
        match permanent::game_directory(&folder) {
            Ok(folder) => {
                self.session = None;
                self.camera = None;
                self.game_dir = Some(folder.clone());
                self.settings.game_dir = Some(folder);
                self.save_settings(self.settings.zoom_limit);
                self.modify_permanent(PermanentAction::Inspect, ctx);
            }
            Err(error) => self.error = Some(self.catalog.error(&error)),
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.connecting = Some(receiver);
        std::thread::spawn(move || {
            let result = GameSession::connect();
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn save_settings(&mut self, zoom_limit: Option<f32>) -> bool {
        let candidate = Settings {
            zoom_limit,
            movement_speed: zoom_limit.map(|_| self.movement_speed),
            ..self.settings.clone()
        };
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
        if matches!(action, PermanentAction::Reset) {
            self.save_after = None;
            if let Some(session) = self.session.as_mut()
                && let Err(error) = session.restore()
            {
                self.error = Some(self.catalog.error(&error));
                return;
            }
        }
        let game_dir = self.game_dir.clone();
        let limit = self.limit;
        let movement_speed = self.movement_speed;
        let features = self.settings.features;
        self.operation_progress.store(0, Ordering::Relaxed);
        let progress = Arc::clone(&self.operation_progress);
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        self.modifying = Some((action, receiver));
        self.error = None;
        std::thread::spawn(move || {
            let result = match action {
                PermanentAction::Inspect => permanent::installed(game_dir.as_deref()),
                PermanentAction::Install => game_dir
                    .as_deref()
                    .ok_or_else(|| GameError::from("permanent_find_game"))
                    .and_then(|game_dir| {
                        permanent::install(game_dir, limit, movement_speed, features, |value| {
                            progress.store(value, Ordering::Relaxed);
                            ctx.request_repaint();
                        })
                    })
                    .and_then(|()| permanent::installed(game_dir.as_deref())),
                PermanentAction::Reset => permanent::reset(game_dir.as_deref()).map(|()| None),
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
            Ok(installed) => {
                self.close_failed = false;
                self.error = None;
                self.installation_unknown = false;
                if matches!(action, PermanentAction::Reset) {
                    self.settings = Settings::default();
                    self.catalog = Catalog::selected(None);
                    self.settings_error = None;
                    self.limit = 300.0;
                    self.movement_speed = 1.0;
                }
                if let Some(installed) = &installed {
                    self.game_dir = Some(installed.game_dir.clone());
                    if self.settings.zoom_limit.is_none() && self.session.is_none() {
                        self.limit = installed.limit;
                        self.movement_speed = installed.movement_speed;
                        self.settings.features = installed.features;
                    }
                }
                self.installed = installed;
            }
            Err(error) => {
                if !matches!(
                    error.key,
                    "error.game_must_close" | "error.permanent_busy" | "permanent_find_game"
                ) {
                    self.installation_unknown = true;
                }
                self.error = Some(self.catalog.error(&error));
            }
        }
        self.next_connect = Instant::now() + RETRY_INTERVAL;
    }

    fn apply(&mut self) {
        // A connection can finish before the debounced disk save.
        self.settings.zoom_limit = Some(self.limit);
        self.settings.movement_speed = Some(self.movement_speed);
        if let Some(session) = self.session.as_mut() {
            match session.apply(self.limit, self.movement_speed, self.settings.features) {
                Ok(()) => {
                    self.error = None;
                }
                Err(error) => self.error = Some(self.catalog.error(&error)),
            }
        }
        self.save_after = Some(Instant::now() + Duration::from_millis(250));
        self.read_camera();
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
                    | "error.mod_unavailable"
                    | "error.live_pending"
            )
        });
        self.next_connect = Instant::now()
            + if transient {
                RETRY_INTERVAL
            } else {
                Duration::from_secs(10)
            };
        self.error = error
            .filter(|error| !transient || error.key == "error.mod_unavailable")
            .map(|error| self.catalog.error(&error));
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
                    self.movement_speed = self.settings.movement_speed.unwrap_or(camera.speed);
                    if self.settings.zoom_limit.is_none() {
                        self.settings.features =
                            self.installed.as_ref().map_or_default(|mod_| mod_.features);
                    }
                    if camera.original > MAX_LIMIT {
                        self.error = Some(self.catalog.text("original_above_cap").to_owned());
                    } else if self.settings.zoom_limit.is_some() {
                        // Persisted limits are applied once for each newly acquired camera.
                        match session.apply(self.limit, self.movement_speed, self.settings.features)
                        {
                            Ok(()) => {
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
            Ok(None) => {
                self.camera = None;
                self.waiting_for_camera = true;
                self.initialize_camera = true;
            }
            Err(error) => self.connection_lost(Some(error)),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_modification();
        if self.save_after.is_some_and(|at| Instant::now() >= at) {
            self.save_after = None;
            self.save_settings(Some(self.limit));
        }
        if self.modifying.is_some() {
            self.read_camera();
            return;
        }
        if let Some(receiver) = &self.connecting {
            match receiver.try_recv() {
                Ok(result) => {
                    self.connecting = None;
                    self.next_connect = Instant::now() + RETRY_INTERVAL;
                    match result {
                        Ok(session) => {
                            let game_dir = session.game_dir.clone();
                            self.game_dir = Some(game_dir.clone());
                            if self.settings.game_dir.as_ref() != Some(&game_dir) {
                                self.settings.game_dir = Some(game_dir);
                                self.save_settings(self.settings.zoom_limit);
                            }
                            match permanent::installed(self.game_dir.as_deref()) {
                                Ok(installed) => {
                                    self.installed = installed;
                                    self.installation_unknown = false;
                                }
                                Err(error) => {
                                    self.installation_unknown = true;
                                    self.error = Some(self.catalog.error(&error));
                                }
                            }
                            self.close_failed = false;
                            self.session = Some(session);
                            self.initialize_camera = true;
                            if !self.installation_unknown {
                                self.error = None;
                            }
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

impl Controller {
    fn tick(&mut self, ctx: &egui::Context) {
        self.poll(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.save_after.take().is_some() {
            self.save_settings(Some(self.limit));
        }
        ctx.request_repaint_after(POLL_INTERVAL);
        if matches!(
            self.modifying,
            Some((PermanentAction::Install | PermanentAction::Reset, _))
        ) && ctx.input(|input| input.viewport().close_requested())
        {
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
}

impl eframe::App for Toolkit {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.controller.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let controller = &mut self.controller;
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(24))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Subspace Toolkit").size(22.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut language = controller.settings.language.clone();
                            egui::ComboBox::from_id_salt("language")
                                .width(90.0)
                                .selected_text(
                                    language
                                        .as_deref()
                                        .unwrap_or(controller.catalog.text("language_auto")),
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut language,
                                        None,
                                        controller.catalog.text("language_auto"),
                                    );
                                    for code in Catalog::languages() {
                                        let label = match code.as_str() {
                                            "de" => "Deutsch",
                                            "en" => "English",
                                            _ => &code,
                                        };
                                        ui.selectable_value(
                                            &mut language,
                                            Some(code.clone()),
                                            label,
                                        );
                                    }
                                });
                            if language != controller.settings.language {
                                controller.select_language(language);
                            }
                        });
                    });
                    let status =
                        if matches!(controller.modifying, Some((PermanentAction::Inspect, _))) {
                            "connecting"
                        } else if controller.modifying.is_some() {
                            "permanent_working"
                        } else if controller.installation_unknown {
                            "permanent_unknown"
                        } else if controller.camera.is_some() {
                            "camera_connected"
                        } else if controller.installed.is_some() {
                            "permanent_installed"
                        } else if controller.connecting.is_some() {
                            "connecting"
                        } else if controller.session.is_some() || controller.waiting_for_camera {
                            "waiting_camera"
                        } else {
                            "waiting_game"
                        };
                    ui.horizontal_wrapped(|ui| {
                        if controller.modifying.is_some() {
                            ui.spinner();
                        }
                        let status_label = ui.strong(controller.catalog.text(status));
                        if let Some(camera) = &controller.camera {
                            status_label.on_hover_text(format!(
                                "{}\n{}",
                                controller.catalog.format(
                                    "readout",
                                    &[
                                        ("current", format!("{:.0}", camera.current)),
                                        ("maximum", format!("{:.0}", camera.maximum))
                                    ]
                                ),
                                controller.catalog.format(
                                    "speed_readout",
                                    &[("speed", format!("{:.0}", camera.speed * 100.0))]
                                )
                            ));
                        }
                        if ui
                            .add_enabled(
                                controller.modifying.is_none(),
                                egui::Button::new(controller.catalog.text("game_folder")),
                            )
                            .clicked()
                            && let Some(folder) = rfd::FileDialog::new().pick_folder()
                        {
                            controller.select_folder(folder, ui.ctx());
                        }
                    });
                    if matches!(controller.modifying, Some((PermanentAction::Install, _))) {
                        ui.add(
                            egui::ProgressBar::new(
                                controller.operation_progress.load(Ordering::Relaxed) as f32
                                    / 100.0,
                            )
                            .show_percentage(),
                        );
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.show_mods,
                            false,
                            controller.catalog.text("camera_tab"),
                        );
                        ui.selectable_value(
                            &mut self.show_mods,
                            true,
                            controller.catalog.text("mods_tab"),
                        );
                    });
                    let minimum = controller
                        .camera
                        .as_ref()
                        .map_or(300.0, |camera| camera.original.min(300.0));
                    let idle = controller.modifying.is_none();
                    let available = controller.camera.is_some() && minimum <= MAX_LIMIT && idle;
                    let slider_available =
                        controller.game_dir.is_some() && minimum <= MAX_LIMIT && idle;
                    if !self.show_mods {
                        let zoom_label = ui.label(controller.catalog.text("zoom_limit"));
                        ui.add_enabled_ui(slider_available, |ui| {
                            ui.spacing_mut().slider_width =
                                (ui.available_width() - 90.0).max(120.0);
                            let slider = ui
                                .add(
                                    egui::Slider::new(
                                        &mut controller.limit,
                                        minimum.min(MAX_LIMIT)..=MAX_LIMIT,
                                    )
                                    .step_by(1.0)
                                    .fixed_decimals(0),
                                )
                                .labelled_by(zoom_label.id);
                            if slider.changed() {
                                controller.apply();
                            }
                        });
                        let speed_label = ui.label(controller.catalog.text("movement_speed"));
                        ui.add_enabled_ui(slider_available, |ui| {
                            ui.spacing_mut().slider_width =
                                (ui.available_width() - 90.0).max(120.0);
                            let slider = ui
                                .add(
                                    egui::Slider::new(&mut controller.movement_speed, 0.25..=3.0)
                                        .step_by(0.05)
                                        .custom_formatter(|n, _| format!("{:.0} %", n * 100.0))
                                        .custom_parser(|s| {
                                            s.trim()
                                                .trim_end_matches('%')
                                                .trim()
                                                .parse::<f64>()
                                                .ok()
                                                .map(|n| n / 100.0)
                                        }),
                                )
                                .labelled_by(speed_label.id);
                            if slider.changed() {
                                controller.apply();
                            }
                        });
                    } else {
                        ui.add_enabled_ui(slider_available, |ui| {
                            let highlight = ui
                                .checkbox(
                                    &mut controller.settings.features.resource_highlight,
                                    controller.catalog.text("resource_highlight"),
                                )
                                .changed();
                            let repeat = ui
                                .checkbox(
                                    &mut controller.settings.features.repeat_build,
                                    controller.catalog.text("repeat_build"),
                                )
                                .changed();
                            let move_buildings = ui
                                .checkbox(
                                    &mut controller.settings.features.move_buildings,
                                    controller.catalog.text("move_buildings"),
                                )
                                .changed();
                            if highlight || repeat || move_buildings {
                                controller.apply();
                            }
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                slider_available
                                    && !controller.installation_unknown
                                    && controller.connecting.is_none(),
                                egui::Button::new(controller.catalog.text("permanent_save")),
                            )
                            .clicked()
                        {
                            controller.modify_permanent(PermanentAction::Install, ui.ctx());
                        }
                        if ui
                            .add_enabled(
                                idle && controller.connecting.is_none()
                                    && (available
                                        || controller.settings.zoom_limit.is_some()
                                        || controller.settings_error.is_some()
                                        || controller.installed.is_some()
                                        || controller.installation_unknown),
                                egui::Button::new(controller.catalog.text("permanent_reset")),
                            )
                            .clicked()
                        {
                            controller.modify_permanent(PermanentAction::Reset, ui.ctx());
                        }
                    });
                    if let Some(detail) = controller
                        .session
                        .as_ref()
                        .and_then(|session| session.feature_error.as_ref())
                    {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            controller
                                .catalog
                                .format("feature_error", &[("detail", detail.clone())]),
                        );
                    }
                    for error in [&controller.error, &controller.settings_error]
                        .into_iter()
                        .flatten()
                    {
                        ui.colored_label(egui::Color32::from_rgb(255, 170, 155), error);
                    }
                    if controller.close_failed {
                        ui.label(controller.catalog.text("close_warning"));
                        if ui.button(controller.catalog.text("close_anyway")).clicked() {
                            controller.force_close = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
            });
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Subspace Toolkit",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([480.0, 390.0])
                .with_min_inner_size([440.0, 320.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Toolkit::new(&cc.egui_ctx)))),
    )
}

#[cfg(test)]
mod controller_tests {
    use super::*;

    #[test]
    fn failed_reset_retains_selection_until_success_and_inspection_preserves_trainer_values() {
        let settings = Settings {
            zoom_limit: Some(700.0),
            movement_speed: Some(0.5),
            features: settings::Features {
                resource_highlight: true,
                repeat_build: true,
                move_buildings: true,
            },
            ..Default::default()
        };
        let mut state = Controller::new(Settings::default(), None);
        state.limit = 700.0;
        state.movement_speed = 0.5;
        state.settings.features = settings.features;
        state.apply();
        assert_eq!(state.settings, settings);
        let complete = |state: &mut Controller, action, result| {
            let (sender, receiver) = mpsc::channel();
            sender.send(result).unwrap();
            state.modifying = Some((action, receiver));
            state.poll_modification();
        };
        complete(
            &mut state,
            PermanentAction::Inspect,
            Ok(Some(InstalledMod {
                game_dir: PathBuf::from("E:/Game"),
                limit: 1000.0,
                movement_speed: 3.0,
                features: Default::default(),
            })),
        );
        assert_eq!((state.limit, state.movement_speed), (700.0, 0.5));
        assert_eq!(state.settings, settings);
        complete(
            &mut state,
            PermanentAction::Reset,
            Err("reset_settings_failed".into()),
        );
        assert_eq!(state.settings, settings);
        assert!(state.error.is_some());
        complete(&mut state, PermanentAction::Reset, Ok(None));
        assert_eq!(state.settings, Settings::default());
        assert_eq!((state.limit, state.movement_speed), (300.0, 1.0));
        assert!(state.error.is_none() && state.installed.is_none());
    }

    #[test]
    fn connection_loss_clears_stale_camera_and_schedules_retry_without_losing_preferences() {
        let settings = Settings {
            zoom_limit: Some(800.0),
            ..Default::default()
        };
        let mut state = Controller::new(settings.clone(), None);
        state.camera = Some(CameraState {
            original: 300.0,
            maximum: 800.0,
            current: 600.0,
            speed: 1.0,
        });
        state.connection_lost(Some("error.mod_unavailable".into()));
        assert!(state.camera.is_none() && state.session.is_none());
        assert_eq!(state.settings, settings);
        assert!(state.next_connect > Instant::now());
        assert!(state.next_connect <= Instant::now() + RETRY_INTERVAL);
    }
}
