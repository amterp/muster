//! Compaction thresholds between the core's words for them and the protocol's.
//!
//! Here because both ends need both directions: a client sends the config's rules and reads
//! what applies to each pane, and the daemon reads the rules, resolves them with the core's
//! resolver and publishes what it found.

use muster_core::compaction::{
    Directory, Effective, Resolved, Rules, Setting, Source, Thresholds, Withheld,
};

use crate as proto;

pub fn rules(rules: &Rules) -> proto::CompactionRules {
    proto::CompactionRules {
        root: sent(rules.root),
        harnesses: rules
            .harnesses
            .iter()
            .map(|(name, set)| proto::CompactionHarness {
                name: name.clone(),
                thresholds: sent(*set),
            })
            .collect(),
        directories: rules
            .directories
            .iter()
            .map(|block| proto::CompactionDirectory {
                path: block.path.clone(),
                thresholds: sent(block.scope),
            })
            .collect(),
    }
}

/// The rules a message holds, or why they are not rules: a percent no agent's context can be
/// at, or a directory that is not absolute or under `~`, which the config file's reader refuses
/// and another client of the daemon could still send.
pub fn read_rules(rules: &proto::CompactionRules) -> Result<Rules, String> {
    let percent = |percent: u32| {
        format!(
            "a compaction rule holds {percent}, and a threshold is a whole percent from 1 to 100"
        )
    };
    if let Some(block) = rules
        .directories
        .iter()
        .find(|block| !muster_core::compaction::is_directory_path(&block.path))
    {
        return Err(format!(
            "a compaction rule's directory is {:?}, and it has to start at `/` or at `~`",
            block.path
        ));
    }
    read_rules_percents(rules).map_err(percent)
}

fn read_rules_percents(rules: &proto::CompactionRules) -> Result<Rules, u32> {
    Ok(Rules {
        root: read_thresholds(rules.root.as_ref())?,
        harnesses: rules
            .harnesses
            .iter()
            .map(|harness| {
                Ok((harness.name.clone(), read_thresholds(harness.thresholds.as_ref())?))
            })
            .collect::<Result<_, u32>>()?,
        directories: rules
            .directories
            .iter()
            .map(|block| {
                Ok(Directory {
                    path: block.path.clone(),
                    scope: read_thresholds(block.thresholds.as_ref())?,
                })
            })
            .collect::<Result<_, u32>>()?,
    })
}

pub fn thresholds(set: &Thresholds) -> proto::CompactionThresholds {
    proto::CompactionThresholds {
        compact_at: set.compact_at.map(threshold),
        nudge_at: set.nudge_at.map(threshold),
    }
}

pub fn threshold(setting: Setting) -> proto::CompactionThreshold {
    proto::CompactionThreshold {
        percent: match setting {
            Setting::Off => None,
            Setting::At(percent) => Some(u32::from(percent)),
        },
    }
}

pub fn read_thresholds(set: Option<&proto::CompactionThresholds>) -> Result<Thresholds, u32> {
    let Some(set) = set else { return Ok(Thresholds::default()) };
    Ok(Thresholds {
        compact_at: set.compact_at.as_ref().map(read_threshold).transpose()?,
        nudge_at: set.nudge_at.as_ref().map(read_threshold).transpose()?,
    })
}

pub fn read_threshold(threshold: &proto::CompactionThreshold) -> Result<Setting, u32> {
    match threshold.percent {
        None => Ok(Setting::Off),
        Some(percent) => u8::try_from(percent)
            .ok()
            .filter(|percent| (1..=100).contains(percent))
            .map(Setting::At)
            .ok_or(percent),
    }
}

pub fn applied(effective: &Effective, withheld: Option<Withheld>) -> proto::AppliedCompaction {
    proto::AppliedCompaction {
        compact_at: Some(applied_threshold(&effective.compact_at)),
        nudge_at: Some(applied_threshold(&effective.nudge_at)),
        nudge_withheld: i32::from(match withheld {
            None => proto::NudgeWithheld::None,
            Some(Withheld::AtOrAboveCompactAt) => proto::NudgeWithheld::AtOrAboveCompactAt,
            Some(Withheld::HarnessCannotCompact) => proto::NudgeWithheld::HarnessCannotCompact,
        }),
    }
}

/// What a daemon said applies to a pane, with why its nudge is withheld. None from a daemon
/// that says nothing of it.
pub fn read_applied(
    applied: Option<&proto::AppliedCompaction>,
) -> Option<(Effective, Option<Withheld>)> {
    let applied = applied?;
    let withheld = match applied.nudge_withheld() {
        proto::NudgeWithheld::None => None,
        proto::NudgeWithheld::AtOrAboveCompactAt => Some(Withheld::AtOrAboveCompactAt),
        proto::NudgeWithheld::HarnessCannotCompact => Some(Withheld::HarnessCannotCompact),
    };
    let effective = Effective {
        compact_at: read_applied_threshold(applied.compact_at.as_ref()),
        nudge_at: read_applied_threshold(applied.nudge_at.as_ref()),
    };
    Some((effective, withheld))
}

fn sent(set: Thresholds) -> Option<proto::CompactionThresholds> {
    (!set.is_empty()).then(|| thresholds(&set))
}

fn applied_threshold(resolved: &Resolved) -> proto::AppliedThreshold {
    let (source, from) = match &resolved.source {
        Source::Unset => (proto::ThresholdSource::Unset, None),
        Source::Root => (proto::ThresholdSource::Root, None),
        Source::Harness(name) => (proto::ThresholdSource::Harness, Some(name.clone())),
        Source::Directory(path) => (proto::ThresholdSource::Directory, Some(path.clone())),
        Source::Pane => (proto::ThresholdSource::Pane, None),
    };
    proto::AppliedThreshold {
        percent: resolved.percent.map(u32::from),
        source: i32::from(source),
        from,
    }
}

fn read_applied_threshold(applied: Option<&proto::AppliedThreshold>) -> Resolved {
    let Some(applied) = applied else {
        return Resolved { percent: None, source: Source::Unset };
    };
    let from = || applied.from.clone().unwrap_or_default();
    Resolved {
        percent: applied.percent.and_then(|percent| u8::try_from(percent).ok()),
        source: match applied.source() {
            proto::ThresholdSource::Unset => Source::Unset,
            proto::ThresholdSource::Root => Source::Root,
            proto::ThresholdSource::Harness => Source::Harness(from()),
            proto::ThresholdSource::Directory => Source::Directory(from()),
            proto::ThresholdSource::Pane => Source::Pane,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_read_back_as_they_were_sent() {
        let sent = Rules {
            root: Thresholds { compact_at: Some(Setting::At(85)), nudge_at: Some(Setting::At(60)) },
            harnesses: [(
                "codex".to_string(),
                Thresholds { compact_at: Some(Setting::Off), nudge_at: None },
            )]
            .into(),
            directories: vec![Directory {
                path: "~/src/muster".to_string(),
                scope: Thresholds { compact_at: None, nudge_at: Some(Setting::At(50)) },
            }],
        };
        assert_eq!(read_rules(&rules(&sent)), Ok(sent));
    }

    #[test]
    fn a_directory_that_could_hold_every_pane_or_none_is_refused() {
        let with = |path: &str| proto::CompactionRules {
            directories: vec![proto::CompactionDirectory {
                path: path.to_string(),
                thresholds: None,
            }],
            ..Default::default()
        };
        assert!(read_rules(&with("")).unwrap_err().contains("directory is \"\""));
        assert!(read_rules(&with("src/muster")).is_err());
        assert!(read_rules(&with("~/src/muster")).is_ok());
    }

    #[test]
    fn a_percent_no_context_can_be_at_is_handed_back() {
        let over = proto::CompactionThreshold { percent: Some(101) };
        assert_eq!(read_threshold(&over), Err(101));
        assert_eq!(read_threshold(&proto::CompactionThreshold { percent: Some(0) }), Err(0));
    }

    #[test]
    fn what_applies_reads_back_with_its_sources() {
        let effective = Effective {
            compact_at: Resolved { percent: None, source: Source::Harness("codex".to_string()) },
            nudge_at: Resolved {
                percent: Some(50),
                source: Source::Directory("~/src/muster".to_string()),
            },
        };
        let withheld = Some(Withheld::HarnessCannotCompact);
        assert_eq!(read_applied(Some(&applied(&effective, withheld))), Some((effective, withheld)));
        assert_eq!(read_applied(None), None);
    }
}
