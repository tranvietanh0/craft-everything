//! The Strongest preset in the app: one switch, on the first run and in Settings → Engines, that
//! runs `settings_apply_preset` (the app never sets those settings itself) and says what happened to
//! Ghidra, including when it couldn't be set up.

use crate::i18n::{tr, trf};
use crate::theme::{self, Tokens};
use crate::{CraftApp, Done, widgets};
use craft_anything_engine::evidence::ToolError;
use craft_anything_engine::{CallOptions, Caller, Consent, Settings};
use egui::{RichText, Ui};
use serde_json::{Value, json};

/// The tool every change to the preset goes through.
pub const TOOL: &str = "settings_apply_preset";

/// The preset being applied, and what the last one said.
#[derive(Debug, Default)]
pub struct PresetFlow {
    /// The `settings_apply_preset` call running, as a job.
    pub job: Option<u64>,
    /// What the last call did, in words, and whether that was a failure (shown under the switch).
    pub outcome: Option<(String, bool)>,
}

impl CraftApp {
    /// Turn the Strongest preset on or off through its tool, on a worker. Turning it on gets
    /// Ghidra when it's missing; that download is the one a Start waits for meanwhile.
    pub fn apply_preset(&mut self, strongest: bool) {
        if self.preset.job.is_some() {
            return;
        }
        if !self.has_tool(TOOL) {
            self.preset.outcome = Some((tr("This build of Craft Anything can't apply settings presets."), true));
            return;
        }
        let gets_ghidra = strongest && !self.ghidra_ready();
        let engine = self.engine.clone();
        let title = if strongest { tr("Use the strongest settings") } else { tr("Use the usual settings") };
        let id = self.jobs.spawn(title, true, move |j| {
            let progress = j.reporter();
            // The click is the person's consent to the download.
            let opts = CallOptions::new(Caller::Ui).with_consent(Consent::Granted).with_cancel(j.cancel.clone()).with_progress(&progress);
            let preset = if strongest { "strongest" } else { "default" };
            let result = engine.call(TOOL, json!({ "preset": preset }), opts).map(|o| o.to_json().get("result").cloned().unwrap_or(Value::Null));
            // What the tool saved, read after it let go of the session.
            let settings = Box::new(engine.session().settings.clone());
            Ok(Done::PresetApplied { result, settings })
        });
        self.preset.job = Some(id);
        self.preset.outcome = None;
        // A Start meanwhile joins this download instead of starting another (see `wait_for_ghidra`).
        if gets_ghidra && self.ghidra.job.is_none() {
            self.ghidra.job = Some(id);
        }
    }

    /// The preset's tool finished: take what it saved, say what happened to Ghidra, and start a
    /// project that was waiting for it (without Ghidra when it couldn't be set up).
    pub fn preset_applied(&mut self, job: u64, result: Result<Value, ToolError>, saved: &Settings) {
        if self.preset.job == Some(job) {
            self.preset.job = None;
        }
        let joined = self.ghidra.job == Some(job);
        if joined {
            self.ghidra.job = None;
        }
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                let words = trf("The settings weren't changed: {error}", &[("error", &e.message)]);
                self.preset.outcome = Some((words.clone(), true));
                self.toast(words, true);
                if joined {
                    self.ghidra_not_got(&e.message);
                }
                return;
            }
        };
        // The tool saved these; the app's own copy follows so its next save keeps them.
        self.settings.deep_mode = saved.deep_mode;
        self.settings.strongest_preset = saved.strongest_preset;
        self.settings.ai = saved.ai.clone();
        self.settings_changed();
        self.ghidra.found = None;
        let ghidra = value.get("ghidra").cloned().unwrap_or(Value::Null);
        let status = ghidra.get("status").and_then(Value::as_str).unwrap_or_default();
        let (words, failed) = match status {
            "installed" => (tr("Strongest settings are on. Ghidra is set up, and Deep mode uses it."), false),
            "already_installed" => (tr("Strongest settings are on. Ghidra was already here, and Deep mode uses it."), false),
            "failed" => {
                let error = ghidra.get("error").and_then(Value::as_str).unwrap_or_default();
                (
                    trf(
                        "Strongest settings are on, but Ghidra couldn't be set up: {error} Deep mode uses Craft Anything's own decompiler until it is.",
                        &[("error", error)],
                    ),
                    true,
                )
            }
            _ => (tr("Back to the usual settings: Clean room and the usual effort. Ghidra stays if it was installed."), false),
        };
        self.preset.outcome = Some((words.clone(), failed));
        self.toast(words, failed);
        if joined {
            if failed {
                let error = ghidra.get("error").and_then(Value::as_str).unwrap_or_default().to_string();
                self.ghidra_not_got(&error);
            } else {
                self.ghidra_got();
            }
        }
    }
}

/// The Strongest switch with what it does, the download while it runs, and what the last change
/// did. The same control on the first run and in Settings → Engines.
pub fn control(app: &mut CraftApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let running = app.preset.job.is_some();
    let mut on = app.settings.strongest_preset;
    let r = ui.add_enabled_ui(!running, |ui| widgets::toggle(ui, &mut on, "Use the strongest settings")).inner;
    if r.changed() {
        app.apply_preset(on);
    }
    ui.add(
        egui::Label::new(
            RichText::new(trf(
                "Deep mode, the builder's highest effort, and Ghidra downloaded (about {mb} MB) so a port never stops to ask. Off: Clean room and the usual effort. AI pictures and sounds stay as you set them.",
                &[("mb", &craft_anything_engines::ghidra::get::DOWNLOAD_MB.to_string())],
            ))
            .size(theme::SMALL)
            .color(t.text_dim),
        )
        .wrap(),
    );
    if let Some(id) = app.preset.job {
        let progress = app.jobs.running().find(|j| j.id == id).and_then(|j| j.progress());
        let (fraction, message) = progress.map(|p| (p.fraction, p.message)).unwrap_or((None, tr("Starting…")));
        ui.add_space(theme::space::XS);
        crate::engine_ui::progress_bar(ui, fraction, &message);
    } else if let Some((words, failed)) = &app.preset.outcome {
        ui.add_space(theme::space::XS);
        let colour = if *failed { t.warning } else { t.text_dim };
        // Said in words, not only by colour.
        ui.add(egui::Label::new(RichText::new(words).size(theme::SMALL).color(colour)).wrap());
    }
}
