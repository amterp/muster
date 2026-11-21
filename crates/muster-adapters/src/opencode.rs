use crate::install::{InstallContext, Plan, Run, asset_word};

pub(crate) fn install(context: &InstallContext<'_>, _: bool, _: &mut Run<'_>) -> Plan {
    let extras = context.extras;
    let readme = asset_word(extras, "opencode/README.md");
    Plan {
        commands: Vec::new(),
        then: vec![
            "OpenCode installs plugins from npm, not from a file, and Muster does not write \
         into its config folder, so this runs nothing. Link the plugin yourself:"
                .to_string(),
            "  mkdir -p ~/.config/opencode/plugin".to_string(),
            format!(
                "  ln -sf {} ~/.config/opencode/plugin/muster.js",
                asset_word(extras, "opencode/plugin/muster.js")
            ),
            format!("{readme} has what it reports."),
        ],
    }
}
