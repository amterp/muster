//! Built-in harness manifests and the adapters distributed with Muster.

use std::io::Write;

use crate::install::{InstallContext, Plan, Run};
use crate::{agy, claude_code, codex, opencode};

#[derive(Debug)]
pub struct Harness {
    pub id: &'static str,
    pub manifest_file: &'static str,
    pub manifest: &'static str,
    pub adapter: Option<Adapter>,
}

#[derive(Debug)]
pub struct Adapter {
    pub dir: &'static str,
    pub name: &'static str,
    /// Required paths relative to the root of extras, including marketplace files.
    pub assets: &'static [&'static str],
    pub install: fn(&InstallContext<'_>, bool, &mut Run<'_>) -> Plan,
    pub install_failure: &'static str,
    pub copy: Option<CopyInspection>,
    pub refresh: Option<Refresh>,
}

#[derive(Debug, Clone, Copy)]
pub struct CopyInspection {
    pub differs: fn(&InstallContext<'_>) -> bool,
    pub warning: &'static str,
    pub install_reminder: &'static str,
}

type RefreshRun = fn(&InstallContext<'_>, &mut dyn Write, &mut Run<'_>) -> Result<(), String>;

#[derive(Debug)]
pub struct Refresh {
    pub run: RefreshRun,
    pub login_repair: &'static str,
    pub at_startup: Option<AutomaticRefresh>,
}

#[derive(Debug, Clone, Copy)]
pub struct AutomaticRefresh {
    pub skipped: &'static str,
    pub impact: &'static str,
    pub repair: &'static str,
}

const CLAUDE_CODE: Adapter = Adapter {
    dir: "claude-code",
    name: "Claude Code",
    assets: &[
        ".claude-plugin/marketplace.json",
        "claude-code/.claude-plugin/plugin.json",
        "claude-code/hooks/hooks.json",
        "claude-code/messaging-hooks.json",
        "claude-code/statusline.sh",
        "claude-code/README.md",
    ],
    install: claude_code::install::install,
    install_failure: " If it says a `muster` marketplace is already added from somewhere else, \
                      such as a checkout of Muster's source, `claude plugin marketplace remove muster` \
                      makes room for this one.",
    copy: None,
    refresh: None,
};

const CODEX: Adapter = Adapter {
    dir: "codex",
    name: "Codex",
    assets: &[
        ".agents/plugins/marketplace.json",
        "codex/.codex-plugin/plugin.json",
        "codex/hooks/hooks.json",
        "codex/messaging-hooks.json",
        "codex/README.md",
    ],
    install: codex::install::install,
    install_failure: " If it says a `muster` marketplace is already added from somewhere else, \
                      such as a checkout of Muster's source, `codex plugin marketplace remove muster` \
                      makes room for this one.",
    copy: Some(CopyInspection {
        differs: codex::install::copy_differs,
        warning: "Codex's copy of Muster's adapter differs from this Muster's: muster harness install codex",
        install_reminder: "Codex keeps its own copy of Muster's adapter: muster harness install codex.",
    }),
    refresh: Some(Refresh {
        run: codex::install::refresh,
        login_repair: "Run `muster harness install codex` from a terminal and review changed hooks in `/hooks`.",
        at_startup: Some(AutomaticRefresh {
            skipped: "this build staged no CLI; no Codex refresh was attempted",
            impact: "Codex's copy of the adapter may still be old; muster window says when it differs",
            repair: "run muster harness install codex, restart Codex and review changed hooks in /hooks",
        }),
    }),
};

const OPENCODE: Adapter = Adapter {
    dir: "opencode",
    name: "OpenCode",
    assets: &["opencode/plugin/muster.js", "opencode/README.md"],
    install: opencode::install,
    install_failure: "",
    copy: None,
    refresh: None,
};

const AGY: Adapter = Adapter {
    dir: "agy",
    name: "Antigravity",
    assets: agy::ASSETS,
    install: agy::install,
    install_failure: "",
    copy: Some(CopyInspection {
        differs: agy::copy_differs,
        warning: "Antigravity's copy of Muster's adapter differs from this Muster's: muster harness refresh agy",
        install_reminder: "Antigravity keeps its own copy of Muster's adapter: muster harness refresh agy.",
    }),
    refresh: Some(Refresh {
        run: agy::refresh,
        login_repair: "Run `muster harness refresh agy` from a terminal.",
        at_startup: None,
    }),
};

macro_rules! bundled {
    ($id:literal, $file:literal) => {
        bundled!(@entry $id, $file, None)
    };
    ($id:literal, $file:literal, $adapter:ident) => {
        bundled!(@entry $id, $file, Some($adapter))
    };
    (@entry $id:literal, $file:literal, $adapter:expr) => {
        Harness {
            id: $id,
            manifest_file: $file,
            manifest: include_str!(concat!("../../muster-detect/manifests/", $file)),
            adapter: $adapter,
        }
    };
}

pub static BUNDLED: &[Harness] = &[
    bundled!("amp", "amp.toml"),
    bundled!("agy", "antigravity.toml", AGY),
    bundled!("claude", "claude.toml", CLAUDE_CODE),
    bundled!("cline", "cline.toml"),
    bundled!("codex", "codex.toml", CODEX),
    bundled!("cursor", "cursor.toml"),
    bundled!("devin", "devin.toml"),
    bundled!("droid", "droid.toml"),
    bundled!("gemini", "gemini.toml"),
    bundled!("copilot", "github-copilot.toml"),
    bundled!("grok", "grok.toml"),
    bundled!("hermes", "hermes.toml"),
    bundled!("kilo", "kilo.toml"),
    bundled!("kimi", "kimi.toml"),
    bundled!("kiro", "kiro.toml"),
    bundled!("maki", "maki.toml"),
    bundled!("opencode", "opencode.toml", OPENCODE),
    bundled!("pi", "pi.toml"),
    bundled!("qodercli", "qodercli.toml"),
];

pub fn adapters() -> impl Iterator<Item = (&'static str, &'static Adapter)> {
    BUNDLED.iter().filter_map(|harness| Some((harness.id, harness.adapter.as_ref()?)))
}

pub fn named(name: &str) -> Option<&'static Adapter> {
    adapters().find(|(id, adapter)| *id == name || adapter.dir == name).map(|(_, adapter)| adapter)
}

pub fn display_names() -> String {
    names(adapters().map(|(_, adapter)| adapter.name), "and")
}

pub fn install_names() -> String {
    names(adapters().map(|(_, adapter)| adapter.dir), "or")
}

fn names(names: impl Iterator<Item = &'static str>, conjunction: &str) -> String {
    let names: Vec<_> = names.collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} {conjunction} {last}", rest.join(", ")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    #[test]
    fn the_catalog_covers_all_shipped_manifests_and_adapters() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entries = |dir: &Path| -> BTreeSet<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        };
        let files: BTreeSet<_> =
            BUNDLED.iter().map(|harness| harness.manifest_file.to_string()).collect();
        assert_eq!(files.len(), BUNDLED.len(), "duplicate manifest file");
        assert_eq!(files, entries(&repo.join("crates/muster-detect/manifests")));
        let ids: BTreeSet<_> = BUNDLED.iter().map(|harness| harness.id).collect();
        assert_eq!(ids.len(), BUNDLED.len(), "duplicate harness ID");

        let extras = repo.join("extras");
        let shipped: BTreeSet<_> = entries(&extras)
            .into_iter()
            .filter(|name| {
                !name.starts_with('.') && !["skill", "api-client-go"].contains(&name.as_str())
            })
            .collect();
        let listed: BTreeSet<_> = adapters().map(|(_, adapter)| adapter.dir.to_string()).collect();
        assert_eq!(listed.len(), adapters().count(), "duplicate adapter directory");
        assert_eq!(shipped, listed);
        for (id, adapter) in adapters() {
            assert_eq!(named(adapter.dir).map(|found| found.dir), Some(adapter.dir));
            assert_eq!(named(id).map(|found| found.dir), Some(adapter.dir));
            assert!(!adapter.assets.is_empty(), "{id} ships no adapter files");
            for file in adapter.assets {
                assert!(extras.join(file).is_file(), "missing extras/{file}");
            }
        }
    }
}
