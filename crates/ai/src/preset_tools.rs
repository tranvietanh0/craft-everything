//! The Strongest preset as one tool: Deep mode on, the builder's maximum effort, and Ghidra set up
//! (with its Java) so a port never stops to ask about it. `default` puts the app's own choices back.
//! Ghidra is never removed by either: it is only installed, and its install is reported honestly.

use crate::AiConfig;
use crate::model::Effort;
use craft_anything_engine::Settings;
use craft_anything_engine::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::ToolError;
use craft_anything_platform::process::Cancel;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        id: "settings_apply_preset",
        title: "Apply a settings preset",
        summary: "Apply `strongest` or `default`. Strongest turns Deep mode on, sets the builder's effort to max for every backend that doesn't set its own, and installs Ghidra (with its Java) when it's missing, so a port never asks about it. Ghidra's install is reported as installed, already_installed or failed; nothing is claimed that didn't happen. Default turns Deep mode and the strongest flag off and clears the default effort. It never removes Ghidra. Settings are saved first, so a failed Ghidra download keeps the choice.",
        beginner: "Sets the app to its strongest settings, or back to normal.",
        toolset: Toolset::Setup,
        effects: Effects::WRITES_FILESYSTEM.union(Effects::MUTATES_SESSION).union(Effects::ACCESSES_NETWORK).union(Effects::LAUNCHES_PROCESS),
        requires: Requires::Nothing,
        input_schema: schema_of::<PresetParams>,
        run: apply,
    }]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Preset {
    Strongest,
    Default,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PresetParams {
    /// `strongest` (Deep mode, max effort, Ghidra installed) or `default` (the app's own choices).
    preset: Preset,
}

fn apply(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PresetParams = parse_params(params)?;
    let strongest = p.preset == Preset::Strongest;
    let next = preset_settings(&ctx.session.settings, strongest)?;
    let path = ctx.session.config_dir().join("config.toml");
    next.save_to(&path)?;
    ctx.session.settings = next;
    let ghidra = if strongest { install_ghidra(ctx) } else { json!({"status": "untouched"}) };
    Ok(ToolOutput::Value(json!({
        "preset": if strongest { "strongest" } else { "default" },
        "deep_mode": strongest,
        "default_effort": if strongest { Effort::Max.as_str() } else { "app default" },
        "ghidra": ghidra,
        "path": path.display().to_string(),
    })))
}

/// The settings with the preset applied: Deep mode, the strongest flag and the default effort set
/// together, so none is on without the others. Nothing is saved here.
fn preset_settings(current: &Settings, strongest: bool) -> Result<Settings, ToolError> {
    let mut next = current.clone();
    let mut ai = AiConfig::from_table(&next.ai)?;
    ai.default_effort = strongest.then_some(Effort::Max);
    next.ai = ai.to_table()?;
    next.deep_mode = strongest;
    next.strongest_preset = strongest;
    Ok(next)
}

/// Install Ghidra (and the Java it runs on) into the engine store when no launcher is found.
/// Failure is a status, not an error: the preset itself is already saved.
fn install_ghidra(ctx: &mut Ctx<'_>) -> Value {
    if ctx.session.settings.ghidra_launcher().is_some() {
        return json!({"status": "already_installed"});
    }
    let store = ctx.session.settings.engine_store_dir();
    let cancel = ctx.cancel.clone();
    let stop: Cancel = Arc::new(move || cancel.is_cancelled());
    let mb = |n: u64| n / (1024 * 1024);
    let progress = |what: &str, got: u64, total: Option<u64>| {
        let fraction = total.filter(|t| *t > 0).map(|t| got as f32 / t as f32);
        ctx.report(fraction, format!("{what}: {} of {} MB", mb(got), total.map_or("?".into(), |t| mb(t).to_string())));
    };
    match craft_anything_engines::ghidra::get::install(&store, Some(stop), &progress) {
        Ok(launcher) => json!({"status": "installed", "launcher": launcher.display().to_string()}),
        Err(e) => json!({"status": "failed", "error": e.message, "remediation": e.remediation}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preset_takes_only_its_two_names() {
        assert!(serde_json::from_value::<PresetParams>(json!({"preset": "strongest"})).is_ok());
        assert!(serde_json::from_value::<PresetParams>(json!({"preset": "default"})).is_ok());
        assert!(serde_json::from_value::<PresetParams>(json!({"preset": "max"})).is_err(), "an unknown preset is refused");
        assert!(serde_json::from_value::<PresetParams>(json!({"preset": "strongest", "extra": 1})).is_err());
    }

    #[test]
    fn strongest_sets_deep_mode_the_flag_and_max_effort_together_and_default_undoes_them() {
        let start = Settings { ai: AiConfig { default_effort: Some(Effort::Low), ..AiConfig::default() }.to_table().unwrap(), ..Settings::default() };
        let on = preset_settings(&start, true).unwrap();
        assert!(on.deep_mode && on.strongest_preset);
        assert_eq!(AiConfig::from_table(&on.ai).unwrap().default_effort, Some(Effort::Max));
        let off = preset_settings(&on, false).unwrap();
        assert!(!off.deep_mode && !off.strongest_preset);
        assert_eq!(AiConfig::from_table(&off.ai).unwrap().default_effort, None, "the default effort is cleared, not set to a level");
    }

    #[test]
    fn an_ai_section_that_cannot_be_read_is_refused_not_overwritten() {
        let bad = Settings { ai: toml::Table::from_iter([("backends".to_string(), toml::Value::Integer(3))]), ..Settings::default() };
        assert!(preset_settings(&bad, true).is_err());
    }
}
