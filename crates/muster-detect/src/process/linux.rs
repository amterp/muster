//! The Linux probe: a walk of the pane's process tree through `/proc/<pid>/task/<tid>/children`,
//! keeping the processes in the foreground group, with `stat`, `cmdline` and `environ` for
//! each.
//!
//! Ported from herdr v0.8.0 `src/platform/linux.rs` (Apache-2.0), and changed: the
//! child-groups fallback (`HERDR_PROCESS_DETECTION=child-groups`) is gone, since a daemon that
//! holds the PTY always has the terminal's own foreground group to hand.
//!
//! Walking the tree rather than scanning all of `/proc` is herdr's choice and a good one: it
//! costs what the pane runs, not what the machine runs.

use std::collections::{HashSet, VecDeque};

#[cfg(target_os = "linux")]
use super::{Job, Process, agent_hint_in};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    pid: u32,
    comm: String,
}

#[cfg(target_os = "linux")]
pub(super) fn job(shell: u32, group: u32) -> Option<Job> {
    let processes: Vec<Process> = members_with(shell, group, task_ids, task_children, live_member)?
        .into_iter()
        .map(|member| Process {
            pid: member.pid,
            name: member.comm,
            argv0: None,
            argv: argv(member.pid),
        })
        .collect();
    Some(Job { group, processes })
}

#[cfg(target_os = "linux")]
pub(super) fn leader(group: u32) -> Option<Job> {
    let (pgrp, name) = pgrp_and_comm(group)?;
    if u32::try_from(pgrp).ok()? != group {
        return None;
    }
    Some(Job {
        group,
        processes: vec![Process { pid: group, name, argv0: None, argv: argv(group) }],
    })
}

#[cfg(target_os = "linux")]
pub(super) fn agent_hint(pid: u32) -> Option<String> {
    agent_hint_in(&std::fs::read(format!("/proc/{pid}/environ")).ok()?)
}

/// The group's members among the processes under `shell`, and under the group's leader in
/// case it is not the shell's descendant, sorted by pid.
fn members_with(
    shell: u32,
    group: u32,
    task_ids: impl FnMut(u32) -> Vec<u32>,
    task_children: impl FnMut(u32, u32) -> Vec<u32>,
    mut live_member: impl FnMut(u32, u32) -> Option<Member>,
) -> Option<Vec<Member>> {
    let mut members: Vec<Member> = tree_pids([shell, group], task_ids, task_children)
        .into_iter()
        .filter_map(|pid| live_member(group, pid))
        .collect();
    members.sort_unstable_by_key(|member| member.pid);
    (!members.is_empty()).then_some(members)
}

fn tree_pids(
    roots: impl IntoIterator<Item = u32>,
    mut task_ids: impl FnMut(u32) -> Vec<u32>,
    mut task_children: impl FnMut(u32, u32) -> Vec<u32>,
) -> Vec<u32> {
    let mut pending = VecDeque::new();
    let mut visited = HashSet::new();
    for pid in roots {
        if pid > 0 && visited.insert(pid) {
            pending.push_back(pid);
        }
    }
    let mut pids = Vec::new();
    while let Some(pid) = pending.pop_front() {
        pids.push(pid);
        // A child is listed under the thread that forked it, so every thread is asked.
        for tid in task_ids(pid) {
            for child in task_children(pid, tid) {
                if child > 0 && visited.insert(child) {
                    pending.push_back(child);
                }
            }
        }
    }
    pids
}

#[cfg(target_os = "linux")]
fn task_ids(pid: u32) -> Vec<u32> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.bytes().all(|byte| byte.is_ascii_digit()).then(|| name.parse().ok()).flatten()
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn task_children(pid: u32, tid: u32) -> Vec<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/children"))
        .map(|children| {
            children.split_whitespace().filter_map(|child| child.parse().ok()).collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn live_member(group: u32, pid: u32) -> Option<Member> {
    let (pgrp, comm) = pgrp_and_comm(pid)?;
    (u32::try_from(pgrp).ok()? == group).then_some(Member { pid, comm })
}

#[cfg(target_os = "linux")]
fn pgrp_and_comm(pid: u32) -> Option<(i32, String)> {
    pgrp_and_comm_from_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// `pid (comm) state ppid pgrp ...`, where comm may itself hold spaces and parentheses - so it
/// runs to the *last* closing parenthesis.
fn pgrp_and_comm_from_stat(stat: &str) -> Option<(i32, String)> {
    let close = stat.rfind(')')?;
    let comm = stat.get(1 + stat.find('(')?..close)?.to_string();
    let fields: Vec<&str> = stat.get(close + 2..)?.split_whitespace().collect();
    Some((fields.get(2)?.parse().ok()?, comm))
}

#[cfg(target_os = "linux")]
fn argv(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let parts: Vec<String> = bytes
        .split(|&byte| byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn foreground_members_follow_the_pane_tree_and_filter_by_process_group() {
        let tasks = HashMap::from([
            (100, vec![100, 101]),
            (200, vec![200]),
            (201, vec![201]),
            (210, vec![210]),
            (220, vec![220]),
            (221, vec![221]),
            (300, vec![300]),
        ]);
        let children = HashMap::from([
            ((100, 100), vec![200, 201, 300]),
            ((100, 101), vec![210]),
            ((200, 200), vec![220]),
            ((220, 220), vec![221]),
        ]);
        let processes = HashMap::from([
            (100, (100, "shell")),
            (200, (200, "leader")),
            (201, (200, "pipeline")),
            (210, (200, "thread-child")),
            (220, (220, "intermediate")),
            (221, (200, "nested-agent")),
            (300, (300, "background")),
            (9999, (200, "unrelated-host-process")),
        ]);
        let task_reads = RefCell::new(Vec::new());
        let child_reads = RefCell::new(Vec::new());
        let member_reads = RefCell::new(Vec::new());

        let members = members_with(
            100,
            200,
            |pid| {
                task_reads.borrow_mut().push(pid);
                tasks.get(&pid).cloned().unwrap_or_default()
            },
            |pid, tid| {
                child_reads.borrow_mut().push((pid, tid));
                children.get(&(pid, tid)).cloned().unwrap_or_default()
            },
            |group, pid| {
                member_reads.borrow_mut().push(pid);
                let (pgrp, comm) = processes.get(&pid)?;
                (*pgrp == group).then(|| Member { pid, comm: (*comm).to_string() })
            },
        )
        .unwrap();

        assert_eq!(
            members.into_iter().map(|member| (member.pid, member.comm)).collect::<Vec<_>>(),
            vec![
                (200, "leader".to_string()),
                (201, "pipeline".to_string()),
                (210, "thread-child".to_string()),
                (221, "nested-agent".to_string()),
            ]
        );
        assert!(child_reads.borrow().contains(&(100, 101)));
        assert!(task_reads.borrow().contains(&220));
        assert!(!task_reads.borrow().contains(&9999));
        assert!(!member_reads.borrow().contains(&9999));
    }

    #[test]
    fn foreground_members_degrade_to_the_direct_group_leader() {
        let members = members_with(
            100,
            200,
            |_| Vec::new(),
            |_, _| Vec::new(),
            |group, pid| (pid == group).then(|| Member { pid, comm: "leader".to_string() }),
        )
        .unwrap();
        assert_eq!(members, vec![Member { pid: 200, comm: "leader".to_string() }]);
    }

    #[test]
    fn foreground_members_observe_new_children_without_a_snapshot_cache() {
        let children = RefCell::new(HashMap::from([((100, 100), vec![200])]));
        let discover = || {
            members_with(
                100,
                200,
                |pid| vec![pid],
                |pid, tid| children.borrow().get(&(pid, tid)).cloned().unwrap_or_default(),
                |group, pid| {
                    [200, 201]
                        .contains(&pid)
                        .then(|| Member { pid, comm: format!("member-{pid}") })
                        .filter(|_| group == 200)
                },
            )
            .unwrap()
            .into_iter()
            .map(|member| member.pid)
            .collect::<Vec<_>>()
        };

        assert_eq!(discover(), vec![200]);
        children.borrow_mut().insert((100, 100), vec![200, 201]);
        assert_eq!(discover(), vec![200, 201]);
    }

    #[test]
    fn proc_stat_parsing_keeps_group_leader_inputs_live() {
        assert_eq!(
            pgrp_and_comm_from_stat("123 (name with ) paren) S 1 456 789 0 456"),
            Some((456, "name with ) paren".to_string()))
        );
    }
}
