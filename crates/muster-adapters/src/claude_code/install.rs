use serde_json::json;

use crate::install::{InstallContext, Plan, Run, Step, asset_word};

pub(crate) fn install(context: &InstallContext<'_>, _: bool, _: &mut Run<'_>) -> Plan {
    let extras = context.extras;
    let extras_path = extras.display().to_string();
    let readme = asset_word(extras, "claude-code/README.md");
    Plan {
        commands: vec![
            Step::of(&["claude", "plugin", "marketplace", "add", &extras_path]),
            Step::of(&["claude", "plugin", "install", "muster@muster"]),
        ],
        then: vec![
            "Claude Code takes a statusline only from its settings, which Muster does not \
         edit. To report context, model and cost, and to name the pane after the \
         session, add this to your settings yourself:"
                .to_string(),
            format!(
                "  \"statusLine\": {{\"type\": \"command\", \"refreshInterval\": 2, \
             \"command\": {}}}",
                json!(asset_word(extras, "claude-code/statusline.sh"))
            ),
            "Sessions already running take the plugin up when they restart.".to_string(),
            format!("{readme} has the statusline and the messaging hooks."),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn the_statusline_path_is_one_shell_word_inside_its_json() {
        let context = InstallContext {
            extras: Path::new("/Users/Jo Smith/.muster/extras"),
            environment: &BTreeMap::new(),
            muster_home: None,
        };
        let plan = install(&context, true, &mut |_, _| panic!("a dry run cannot execute commands"));
        assert!(
            plan.then.iter().any(|line| line.contains(
                r#""command": "'/Users/Jo Smith/.muster/extras/claude-code/statusline.sh'""#
            )),
            "{:?}",
            plan.then
        );
    }
}
