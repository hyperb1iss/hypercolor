//! launchd itself, driving a fixture agent in this user's gui domain: the
//! facts a bundle installation builds on, against the real launchctl.

#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use hypercolor_macos_owner::{
    MacosLaunchdJob, parse_launchd_autostart_state, parse_launchd_job, wait_for_process_exit_within,
};

struct FixtureAgent {
    label: String,
    uid: u32,
    plist: tempfile::TempPath,
}

impl FixtureAgent {
    fn new() -> Self {
        let uid = String::from_utf8(run("/usr/bin/id", &["-u"]).stdout)
            .expect("id prints UTF-8")
            .trim()
            .parse()
            .expect("numeric uid");
        let label = format!(
            "tech.hyperbliss.hypercolor.launchd-fixture.{}",
            std::process::id()
        );
        let plist = tempfile::Builder::new()
            .suffix(".plist")
            .tempfile()
            .expect("plist file");
        std::fs::write(
            plist.path(),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array><string>/bin/sleep</string><string>120</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ThrottleInterval</key><integer>30</integer>
</dict></plist>
"#
            ),
        )
        .expect("write plist");
        Self {
            label,
            uid,
            plist: plist.into_temp_path(),
        }
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }

    fn launchctl(&self, arguments: &[&str]) -> Output {
        run("/bin/launchctl", arguments)
    }

    fn job(&self) -> MacosLaunchdJob {
        let output = self.launchctl(&["print", &self.target()]);
        parse_launchd_job(
            &self.label,
            self.uid,
            output.status.code(),
            &output.stdout,
            &output.stderr,
        )
        .expect("exact launchctl print")
    }

    fn autostart_enabled(&self) -> bool {
        let output = self.launchctl(&["print-disabled", &self.domain()]);
        assert!(output.status.success(), "print-disabled");
        parse_launchd_autostart_state(&self.label, &output.stdout).expect("exact print-disabled")
    }

    fn bootstrap(&self) -> Output {
        let plist = self.plist.to_str().expect("UTF-8 plist path");
        self.launchctl(&["bootstrap", &self.domain(), plist])
    }

    fn wait_for_job(&self, matches: impl Fn(MacosLaunchdJob) -> bool) -> MacosLaunchdJob {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let job = self.job();
            if matches(job) {
                return job;
            }
            assert!(Instant::now() < deadline, "launchd stayed at {job:?}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for FixtureAgent {
    fn drop(&mut self) {
        let target = self.target();
        self.launchctl(&["bootout", &target]);
        self.launchctl(&["enable", &target]);
    }
}

fn run(program: &str, arguments: &[&str]) -> Output {
    Command::new(program)
        .args(arguments)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("run command")
}

fn running_pid(job: MacosLaunchdJob) -> u32 {
    match job {
        MacosLaunchdJob::Running { pid } => pid,
        other => panic!("expected a running job, found {other:?}"),
    }
}

#[test]
fn launchd_gates_loading_on_the_disabled_state_and_never_a_loaded_job() {
    let agent = FixtureAgent::new();
    assert!(Path::new(&agent.plist).is_file());
    assert_eq!(agent.job(), MacosLaunchdJob::NotLoaded);
    assert!(agent.autostart_enabled(), "a label starts enabled");

    let bootstrap = agent.bootstrap();
    assert!(bootstrap.status.success(), "{bootstrap:?}");
    let pid = running_pid(agent.wait_for_job(|job| matches!(job, MacosLaunchdJob::Running { .. })));

    // Disabling never touches what already runs.
    let target = agent.target();
    assert!(agent.launchctl(&["disable", &target]).status.success());
    assert!(!agent.autostart_enabled());
    assert_eq!(agent.job(), MacosLaunchdJob::Running { pid });
    assert!(
        !wait_for_process_exit_within(pid, Duration::from_millis(200)).expect("watch"),
        "the job outlives the short window"
    );

    // A job that dies exits at once for the watcher, and launchd keeps the
    // job loaded and idle until its throttle lets it restart.
    assert!(
        agent
            .launchctl(&["kill", "SIGKILL", &target])
            .status
            .success()
    );
    assert!(
        wait_for_process_exit_within(pid, Duration::from_secs(10)).expect("watch"),
        "NOTE_EXIT reports launchd's own job"
    );
    assert_eq!(
        agent.wait_for_job(|job| job != MacosLaunchdJob::Running { pid }),
        MacosLaunchdJob::Idle,
        "a crashed job stays loaded, waiting out its throttle"
    );

    assert!(agent.launchctl(&["bootout", &target]).status.success());
    assert_eq!(agent.job(), MacosLaunchdJob::NotLoaded);

    // The gate holds across the unload: nothing loads until it reopens.
    let refused = agent.bootstrap();
    assert!(
        !refused.status.success(),
        "launchd must refuse a disabled agent: {refused:?}"
    );
    assert_eq!(agent.job(), MacosLaunchdJob::NotLoaded);

    assert!(agent.launchctl(&["enable", &target]).status.success());
    assert!(agent.autostart_enabled());
    let bootstrap = agent.bootstrap();
    assert!(bootstrap.status.success(), "{bootstrap:?}");
    agent.wait_for_job(|job| matches!(job, MacosLaunchdJob::Running { .. }));
}
