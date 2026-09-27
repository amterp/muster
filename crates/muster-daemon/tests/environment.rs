//! What a pane's programs are told about the terminal they run in (MIP-3 section 3).

mod support;

use support::*;

#[test]
fn a_pane_runs_as_xterm_ghostty_from_the_daemons_own_terminfo() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = daemon.root().join("infocmp");
    let mut asked = create("p1", in_new_tab("t1"));
    asked.command = Some(format!(
        "{{ infocmp -x; echo \"$TERM_PROGRAM $TERM_PROGRAM_VERSION\"; echo end; }} > {} 2>&1",
        out.display()
    ));
    make(&mut control, asked);
    let written = until_some("the pane to describe its terminal", || {
        std::fs::read_to_string(&out).ok().filter(|text| text.ends_with("end\n"))
    });

    // The first line says which file the entry came from: the daemon's, and not a system copy
    // that happens to exist on this machine.
    let data = std::path::Path::new(DAEMON_DATA).canonicalize().unwrap();
    let source = written.lines().next().unwrap_or_default();
    assert!(
        source.contains(&data.join("terminfo").display().to_string()),
        "infocmp read {source:?}\n{written}"
    );
    assert!(written.contains("\nxterm-ghostty|ghostty|Ghostty,\n"), "{written}");
    assert!(written.contains("\nghostty "), "TERM_PROGRAM: {written}");
}

#[test]
fn a_daemon_without_its_data_directory_refuses_to_start() {
    let scratch = std::env::temp_dir().join(format!("muster-no-data-{}", std::process::id()));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_muster-daemon"))
        .arg("--socket")
        .arg(scratch.join("daemon.sock"))
        .arg("--data")
        .arg(&scratch)
        .env_clear()
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "it started: {stderr}");
    assert!(stderr.contains(&format!("{} is not a complete", scratch.display())), "{stderr}");
    assert!(!scratch.join("daemon.sock").exists(), "it claimed the socket anyway");
}
