use std::os::unix::fs::PermissionsExt;

use muster::proto::{Request, Response, Startup, request, response};
use muster_harness::{Daemon, until};
use prost::Message;

fn launch_refresh(fails: bool) -> serde_json::Value {
    let daemon = Daemon::start_built();
    let scratch = daemon.muster_config().with_file_name("adapter-refresh");
    std::fs::create_dir_all(&scratch).unwrap();
    let cli = scratch.join("muster");
    let invoked = scratch.join("invoked");
    let log = scratch.join("run.jsonl");
    let finish = if fails {
        "echo 'Codex marketplace query timed out' >&2; exit 1"
    } else {
        "echo 'Codex plugin matches this Muster'"
    };
    std::fs::write(
        &cli,
        format!("#!/bin/sh\nprintf '%s' \"$*\" > '{}'\n{finish}\n", invoked.display()),
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = Request {
        payload: Some(request::Payload::Startup(Startup {
            config_path: daemon.muster_config().display().to_string(),
            commands_path: scratch.display().to_string(),
            log_path: log.display().to_string(),
            ..Startup::default()
        })),
        ..Request::default()
    };
    let answered = Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice()).unwrap();
    assert!(matches!(answered.payload, Some(response::Payload::Ok(_))), "{answered:?}");
    let event = if fails { "adapter.refresh.failed" } else { "adapter.refresh.finished" };
    let mut recorded = None;
    until(
        "startup logs its adapter refresh outcome",
        || {
            recorded = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|line| line["event"] == event);
            recorded.is_some()
        },
        || std::fs::read_to_string(&log).unwrap_or_default(),
    );
    assert_eq!(std::fs::read_to_string(invoked).unwrap(), "harness refresh codex --login");
    recorded.unwrap()
}

#[test]
fn startup_refreshes_adapters_and_logs_the_outcome() {
    let _turn = muster::testing::fresh_session();
    let record = launch_refresh(false);
    assert_eq!(record["level"], "info");
    assert_eq!(record["outcome"], "Codex plugin matches this Muster");
}

#[test]
fn a_failed_adapter_refresh_warns_with_its_impact_and_fix() {
    let _turn = muster::testing::fresh_session();
    let record = launch_refresh(true);
    assert_eq!(record["level"], "warn");
    assert!(record["reason"].as_str().unwrap().contains("Codex marketplace query timed out"));
    assert!(record["impact"].as_str().unwrap().contains("old"));
    assert!(record["fix"].as_str().unwrap().contains("muster harness install codex"));
}
