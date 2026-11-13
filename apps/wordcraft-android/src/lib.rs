//! WordCraft as an Android app (phones, tablets and ChromeOS, which runs Android apps natively).
//!
//! Runs the same [`wordcraft_ui_egui::WordApp`] as the desktop app through eframe's Android runner
//! (winit NativeActivity, wgpu on Vulkan or GLES), in one window as on the web: Android has no
//! separate desktop windows. File dialogs are WordCraft's own folder browser ([`browser`]) over the
//! folders the user let it use ([`files`]). Build: `.github/workflows/android.yml`.
#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod browser;
mod files;
mod page;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, UiState, WordApp};

struct App {
    app: WordApp,
    browser: browser::Browser,
    pending: browser::Pending,
    files: Rc<RefCell<files::Files>>,
    prefs: Option<PathBuf>,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Documents ChromeOS handed over ("Open with", or WordCraft as the default app).
        let opened = self.files.borrow_mut().take_opened();
        for o in opened {
            match o {
                Ok((path, _, name, writable)) => {
                    if let Err(e) = self.app.open_from_platform(&path, None) {
                        self.app.report_open_failure(&path, &e);
                    } else if !writable {
                        self.app.status_long(format!("{name} was opened read only. Use Save As to keep your changes."));
                    }
                }
                Err(e) => self.app.status_long(e),
            }
        }
        let problems = self.files.borrow_mut().logic();
        if let Some(p) = problems.into_iter().last() {
            self.app.status_long(p);
        }
        self.app.logic(ctx);
    }
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
        self.browser.show(ui.ctx(), &self.pending, &self.files);
    }
    fn on_exit(&mut self) {
        let _ = self.files.borrow_mut().sync_back();
        save_prefs(&self.app, self.prefs.as_deref());
    }
}

fn save_prefs(app: &WordApp, path: Option<&Path>) {
    let Some(path) = path else { return };
    if let Ok(b) = serde_json::to_vec_pretty(&app.prefs()) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[unsafe(no_mangle)]
fn android_main(android_app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("wordcraft"));
    files::set_activity(&android_app);
    let data = android_app.internal_data_path();
    let prefs = data.as_ref().map(|d| d.join("ui.json"));
    let files = Rc::new(RefCell::new(files::Files::new(data.clone())));
    let pending: browser::Pending = Rc::new(RefCell::new(None));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("WordCraft"),
        android_app: Some(android_app),
        ..Default::default()
    };
    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(move |cc| {
            files::set_context(&cc.egui_ctx);
            let region = wordcraft_ui_egui::i18n::system_region();
            let mut session = Session::new(wordcraft_engine::blank_document(&region));
            session.system_region = region;
            let pending_hook = pending.clone();
            let services = Services {
                file_dialog: Some(browser::hook(pending_hook, cc.egui_ctx.clone())),
                cancel_file_dialog: Some(Box::new({
                    let pending = pending.clone();
                    move || {
                        pending.borrow_mut().take();
                    }
                })),
                ..Default::default()
            };
            let mut app = WordApp::new(session, services);
            if let Some(p) = &prefs
                && let Ok(bytes) = std::fs::read(p)
                && let Some(ui) = UiState::from_json(&bytes)
            {
                app.apply_prefs(ui);
            }
            if app.session.prefs.region.is_some() {
                let doc = app.session.blank_document();
                app.replace_startup_document(doc);
            }
            app.start_screen();
            Ok(Box::new(App { app, browser: browser::Browser::default(), pending, files, prefs }))
        }),
    );
    if let Err(e) = result {
        log::error!("WordCraft stopped: {e}");
    }
}
