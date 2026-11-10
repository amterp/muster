//! Which compaction thresholds apply to one pane.
//!
//! Two thresholds, each a percent of an agent's context: past `compact_at` its daemon compacts
//! it, and past `compact_nudge_at` its daemon tells it to compact itself. Either can be set for
//! every agent, for a harness, for a directory, and for one pane, and the most specific answer
//! wins, each key on its own.
//!
//! Pure and here rather than in the daemon, so the config file's reader and the daemon that
//! applies it resolve with the same code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What one place says about one threshold. A place that says nothing holds no `Setting`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// `"none"`: off here, whatever a less specific place says.
    Off,
    /// A whole percent from 1 to 100.
    At(u8),
}

/// What one place says about both thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Thresholds {
    pub compact_at: Option<Setting>,
    pub nudge_at: Option<Setting>,
}

impl Thresholds {
    pub fn is_empty(&self) -> bool {
        self.compact_at.is_none() && self.nudge_at.is_none()
    }
}

/// A `[[directory]]` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    /// As the file wrote it, less any trailing slash: absolute, or under `~`.
    pub path: String,
    pub scope: Thresholds,
}

/// Everything a config file says about compaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    pub root: Thresholds,
    /// By the harness's name as its detection manifest spells it (`claude`, `codex`).
    pub harnesses: BTreeMap<String, Thresholds>,
    pub directories: Vec<Directory>,
}

/// Where a pane's answer for one threshold came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Nothing sets it.
    Unset,
    Root,
    /// The harness's name.
    Harness(String),
    /// The block's path, as the file wrote it.
    Directory(String),
    Pane,
}

/// One threshold as it applies to one pane: the percent, or none for off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub percent: Option<u8>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub compact_at: Resolved,
    pub nudge_at: Resolved,
}

/// Why a pane with a nudge threshold is never nudged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Withheld {
    /// The nudge would arrive with the compaction or after it.
    AtOrAboveCompactAt,
    /// The nudge tells an agent to compact itself, and this harness gives no way to.
    HarnessCannotCompact,
}

impl Rules {
    /// The one percent every agent is compacted at, when that is the whole of what the rules say
    /// about `compact_at`. None when nothing is compacted, and also when a harness or a
    /// directory says anything about it: a daemon that takes a single percent would compact
    /// agents these rules exempt.
    pub fn compact_at_everywhere(&self) -> Option<u8> {
        let scoped = self.harnesses.values().any(|scope| scope.compact_at.is_some())
            || self.directories.iter().any(|block| block.scope.compact_at.is_some());
        match self.root.compact_at {
            Some(Setting::At(percent)) if !scoped => Some(percent),
            _ => None,
        }
    }

    /// Whether the rules say anything a single `compact_at` percent cannot.
    pub fn needs_scopes(&self) -> bool {
        self.root.nudge_at.is_some() || !self.harnesses.is_empty() || !self.directories.is_empty()
    }

    /// The thresholds for a pane running `harness` in `cwd`, on a machine whose home is `home`.
    ///
    /// For each key: the pane's own setting, then each directory block holding `cwd`, longest
    /// path first, then the harness's table, then the root. A place that omits the key passes
    /// the question on; `Setting::Off` answers it.
    pub fn resolve(
        &self,
        pane: &Thresholds,
        harness: Option<&str>,
        cwd: &Path,
        home: &Path,
    ) -> Effective {
        let mut holding: Vec<(usize, &Directory)> = self
            .directories
            .iter()
            .filter_map(|block| {
                let path = expanded(&block.path, home);
                cwd.starts_with(&path).then(|| (path.components().count(), block))
            })
            .collect();
        holding.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
        let harness = harness.and_then(|name| Some((name, self.harnesses.get(name)?)));

        let resolve = |key: fn(&Thresholds) -> Option<Setting>| {
            let (setting, source) = key(pane)
                .map(|setting| (setting, Source::Pane))
                .or_else(|| {
                    holding.iter().find_map(|(_, block)| {
                        Some((key(&block.scope)?, Source::Directory(block.path.clone())))
                    })
                })
                .or_else(|| {
                    let (name, scope) = harness?;
                    Some((key(scope)?, Source::Harness(name.to_string())))
                })
                .or_else(|| Some((key(&self.root)?, Source::Root)))
                .unzip();
            let percent = match setting {
                Some(Setting::At(percent)) => Some(percent),
                Some(Setting::Off) | None => None,
            };
            Resolved { percent, source: source.unwrap_or(Source::Unset) }
        };
        Effective {
            compact_at: resolve(|scope| scope.compact_at),
            nudge_at: resolve(|scope| scope.nudge_at),
        }
    }
}

impl Effective {
    /// Why this pane's nudge will never be sent, if it has one and it will not. `compacts` is
    /// whether the pane's harness gives Muster a way to compact it.
    pub fn nudge_withheld(&self, compacts: bool) -> Option<Withheld> {
        let nudge_at = self.nudge_at.percent?;
        if !compacts {
            return Some(Withheld::HarnessCannotCompact);
        }
        self.compact_at
            .percent
            .is_some_and(|compact_at| nudge_at >= compact_at)
            .then_some(Withheld::AtOrAboveCompactAt)
    }
}

/// A `[[directory]]` path with a leading `~` replaced by `home`.
fn expanded(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/somebody";

    fn at(compact_at: Option<Setting>, nudge_at: Option<Setting>) -> Thresholds {
        Thresholds { compact_at, nudge_at }
    }

    fn directory(path: &str, scope: Thresholds) -> Directory {
        Directory { path: path.to_string(), scope }
    }

    fn resolve(rules: &Rules, pane: &Thresholds, harness: Option<&str>, cwd: &str) -> Effective {
        rules.resolve(pane, harness, Path::new(cwd), Path::new(HOME))
    }

    fn rules() -> Rules {
        Rules {
            root: at(Some(Setting::At(85)), Some(Setting::At(60))),
            harnesses: BTreeMap::from([
                ("claude".to_string(), at(None, Some(Setting::At(55)))),
                ("codex".to_string(), at(Some(Setting::Off), Some(Setting::Off))),
            ]),
            directories: vec![
                directory("~/src", at(Some(Setting::At(90)), None)),
                directory("~/src/muster", at(None, Some(Setting::At(50)))),
            ],
        }
    }

    #[test]
    fn nothing_set_is_off_and_says_so() {
        let effective = resolve(&Rules::default(), &Thresholds::default(), Some("claude"), "/tmp");
        assert_eq!(effective.compact_at, Resolved { percent: None, source: Source::Unset });
        assert_eq!(effective.nudge_at, Resolved { percent: None, source: Source::Unset });
    }

    #[test]
    fn the_root_answers_for_a_harness_and_a_directory_nothing_names() {
        let effective = resolve(&rules(), &Thresholds::default(), Some("opencode"), "/tmp");
        assert_eq!(effective.compact_at, Resolved { percent: Some(85), source: Source::Root });
        assert_eq!(effective.nudge_at, Resolved { percent: Some(60), source: Source::Root });
    }

    #[test]
    fn a_harness_table_beats_the_root_key_by_key() {
        let effective = resolve(&rules(), &Thresholds::default(), Some("claude"), "/tmp");
        assert_eq!(effective.compact_at.source, Source::Root, "the table omits compact_at");
        assert_eq!(
            effective.nudge_at,
            Resolved { percent: Some(55), source: Source::Harness("claude".to_string()) }
        );
    }

    #[test]
    fn none_turns_a_root_value_off_for_a_harness() {
        let effective = resolve(&rules(), &Thresholds::default(), Some("codex"), "/tmp");
        let codex = Source::Harness("codex".to_string());
        assert_eq!(effective.compact_at, Resolved { percent: None, source: codex.clone() });
        assert_eq!(effective.nudge_at, Resolved { percent: None, source: codex });
    }

    #[test]
    fn the_longest_directory_wins_and_a_key_it_omits_falls_to_a_shorter_one() {
        let cwd = "/Users/somebody/src/muster/crates";
        let effective = resolve(&rules(), &Thresholds::default(), Some("codex"), cwd);
        assert_eq!(
            effective.nudge_at,
            Resolved { percent: Some(50), source: Source::Directory("~/src/muster".to_string()) }
        );
        assert_eq!(
            effective.compact_at,
            Resolved { percent: Some(90), source: Source::Directory("~/src".to_string()) },
            "~/src/muster says nothing about compact_at, and ~/src beats codex's table"
        );
    }

    #[test]
    fn a_directory_matches_whole_path_components() {
        let effective =
            resolve(&rules(), &Thresholds::default(), None, "/Users/somebody/src/muster-4");
        assert_eq!(effective.nudge_at.source, Source::Root, "muster-4 is not inside muster");
        assert_eq!(effective.compact_at.source, Source::Directory("~/src".to_string()));
    }

    #[test]
    fn a_pane_setting_beats_everything_and_can_turn_a_threshold_off() {
        let pane = at(Some(Setting::Off), Some(Setting::At(70)));
        let effective = resolve(&rules(), &pane, Some("claude"), "/Users/somebody/src/muster");
        assert_eq!(effective.compact_at, Resolved { percent: None, source: Source::Pane });
        assert_eq!(effective.nudge_at, Resolved { percent: Some(70), source: Source::Pane });
    }

    #[test]
    fn an_absolute_path_and_the_home_itself_are_directories_too() {
        let rules = Rules {
            directories: vec![
                directory("~", at(None, Some(Setting::At(40)))),
                directory("/srv/work", at(None, Some(Setting::At(45)))),
            ],
            ..Rules::default()
        };
        let at_home = resolve(&rules, &Thresholds::default(), None, "/Users/somebody/notes");
        assert_eq!(at_home.nudge_at.percent, Some(40));
        let at_work = resolve(&rules, &Thresholds::default(), None, "/srv/work/api");
        assert_eq!(at_work.nudge_at.percent, Some(45));
        let elsewhere = resolve(&rules, &Thresholds::default(), None, "");
        assert_eq!(
            elsewhere.nudge_at.source,
            Source::Unset,
            "a pane with no directory matches none"
        );
    }

    #[test]
    fn one_percent_everywhere_is_only_said_of_rules_that_scope_no_compaction() {
        let mut rules = Rules { root: at(Some(Setting::At(85)), None), ..Rules::default() };
        assert_eq!(rules.compact_at_everywhere(), Some(85));
        assert!(!rules.needs_scopes());

        rules.harnesses.insert("claude".to_string(), at(None, Some(Setting::At(55))));
        assert_eq!(rules.compact_at_everywhere(), Some(85), "the table scopes only the nudge");
        assert!(rules.needs_scopes());

        rules.directories.push(directory("~/src", at(Some(Setting::Off), None)));
        assert_eq!(rules.compact_at_everywhere(), None, "85 everywhere would compact ~/src");

        let off = Rules { root: at(Some(Setting::Off), None), ..Rules::default() };
        assert_eq!(off.compact_at_everywhere(), None);
    }

    #[test]
    fn a_nudge_is_withheld_at_or_above_the_compaction_and_from_a_harness_that_cannot_compact() {
        let effective = |compact_at, nudge_at| Effective {
            compact_at: Resolved { percent: compact_at, source: Source::Root },
            nudge_at: Resolved { percent: nudge_at, source: Source::Root },
        };
        assert_eq!(effective(Some(85), Some(60)).nudge_withheld(true), None);
        assert_eq!(effective(None, Some(60)).nudge_withheld(true), None);
        assert_eq!(effective(Some(85), None).nudge_withheld(false), None, "no nudge to withhold");
        assert_eq!(
            effective(Some(85), Some(85)).nudge_withheld(true),
            Some(Withheld::AtOrAboveCompactAt)
        );
        assert_eq!(
            effective(Some(85), Some(60)).nudge_withheld(false),
            Some(Withheld::HarnessCannotCompact)
        );
    }
}
