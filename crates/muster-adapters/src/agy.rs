use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::install::{InstallContext, Plan, REFRESH_PATIENCE, Run, Step, asset_word};

const PLUGIN_NAME: &str = "muster";
const MANIFEST: &str = "agy/plugin.json";
const STATUS_LINE: &str = "agy/statusline.sh";
const README: &str = "agy/README.md";
pub(crate) const ASSETS: &[&str] =
    &[MANIFEST, "agy/hooks.json", "agy/report.py", STATUS_LINE, README];

pub(crate) fn install(context: &InstallContext<'_>, _: bool, _: &mut Run<'_>) -> Plan {
    let readme = asset_word(context.extras, README);
    Plan {
        commands: vec![install_step(context.extras)],
        then: vec![
            "Restart Antigravity to load the plugin. Its hooks and status line need Python 3."
                .to_string(),
            "Set the status line through your settings or configuration manager:".to_string(),
            format!(
                "  \"statusLine\": {{\"command\": {}, \"stack_with_default\": true}}",
                json!(asset_word(context.extras, STATUS_LINE))
            ),
            format!("{readme} shows how to keep your existing status line."),
        ],
    }
}

fn install_step(extras: &Path) -> Step {
    Step::of(&["agy", "plugin", "install", &extras.join("agy").display().to_string()])
}

fn plugins_dir(context: &InstallContext<'_>) -> Option<PathBuf> {
    let home = context.environment.get("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(home).join(".gemini/config/plugins"))
}

fn copy_path(root: &Path, installed: &Path) -> Option<PathBuf> {
    if !installed.is_absolute() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let installed = installed.canonicalize().ok()?;
    (installed != root && installed.starts_with(root) && installed.is_dir()).then_some(installed)
}

fn differs(extras: &Path, copy: &Path) -> Result<bool, String> {
    let mut differs = false;
    for relative in ASSETS.iter().copied().filter(|asset| *asset != README) {
        let source = extras.join(relative);
        let contents = std::fs::read(&source).map_err(|error| {
            format!("could not read bundled file {}: {error}", source.display())
        })?;
        let name = relative.strip_prefix("agy/").expect("plugin asset is relative to extras");
        differs |= std::fs::read(copy.join(name)).ok().as_deref() != Some(&contents);
    }
    Ok(differs)
}

pub(crate) fn copy_differs(context: &InstallContext<'_>) -> bool {
    let Some(root) = plugins_dir(context) else { return false };
    copy_path(&root, &root.join(PLUGIN_NAME))
        .is_some_and(|copy| differs(context.extras, &copy).unwrap_or(false))
}

struct Installed {
    path: PathBuf,
    enabled: bool,
}

impl Installed {
    fn disabled_note(&self) -> &str {
        if self.enabled { "" } else { " The plugin is disabled." }
    }
}

pub(crate) fn refresh(
    context: &InstallContext<'_>,
    out: &mut dyn Write,
    run: &mut Run<'_>,
) -> Result<(), String> {
    let root = plugins_dir(context).ok_or("could not locate Antigravity's plugins directory")?;
    let Some(before) = installed(&root, run)? else {
        let _ = writeln!(out, "Antigravity plugin not installed. Nothing to refresh.");
        return Ok(());
    };
    if !differs(context.extras, &before.path)? {
        let _ = writeln!(out, "Antigravity plugin matches this Muster.{}", before.disabled_note());
        return Ok(());
    }
    let manifest = std::fs::read(context.extras.join(MANIFEST))
        .map_err(|error| format!("could not read the bundled Antigravity plugin: {error}"))?;
    let plugin: Value = serde_json::from_slice(&manifest)
        .map_err(|error| format!("invalid bundled Antigravity plugin: {error}"))?;
    if plugin["name"] != PLUGIN_NAME {
        return Err(format!("the bundled Antigravity plugin must be named {PLUGIN_NAME}"));
    }
    command(&install_step(context.extras), run)?;
    let after = installed(&root, run)?
        .ok_or("Antigravity reported success but its Muster plugin is missing")?;
    if after.enabled != before.enabled {
        return Err("Antigravity's plugin enabled state changed during refresh.".to_string());
    }
    if differs(context.extras, &after.path)? {
        return Err(
            "Antigravity reported success but its plugin copy still differs from this Muster."
                .to_string(),
        );
    }
    let _ = writeln!(
        out,
        "Antigravity plugin refreshed. Restart Antigravity to load it.{}",
        after.disabled_note()
    );
    Ok(())
}

fn installed(root: &Path, run: &mut Run<'_>) -> Result<Option<Installed>, String> {
    let listing = command(&Step::of(&["agy", "plugin", "list", "--json"]), run)?;
    let entries: Vec<Value> = serde_json::from_slice(&listing.stdout)
        .map_err(|error| format!("invalid Antigravity plugin listing: {error}"))?;
    let mut result = None;
    for entry in entries {
        let name = entry["name"].as_str().ok_or("Antigravity listed a plugin without a name")?;
        if name != PLUGIN_NAME {
            continue;
        }
        if result.is_some() {
            return Err("Antigravity listed the Muster plugin twice.".to_string());
        }
        let path = entry["path"].as_str().ok_or("Antigravity listed Muster without a path")?;
        let path = copy_path(root, Path::new(path)).ok_or(
            "Antigravity listed Muster outside its plugins directory or at an invalid path",
        )?;
        let enabled = entry["enabled"]
            .as_bool()
            .ok_or("Antigravity did not say whether Muster is enabled")?;
        result = Some(Installed { path, enabled });
    }
    Ok(result)
}

fn command(step: &Step, run: &mut Run<'_>) -> Result<Output, String> {
    let failed = |reason: &str| format!("`{}` failed ({reason})", step.line());
    let output = run(&step.words, REFRESH_PATIENCE)
        .map_err(|error| failed(&error.to_string()))?
        .ok_or_else(|| failed("it timed out after 10 seconds and was stopped"))?;
    if !output.status.success() {
        return Err(failed(&format!(
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}
