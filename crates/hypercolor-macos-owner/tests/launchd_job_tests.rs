//! launchd itself, driving a fixture agent in this user's gui domain: the
//! facts a bundle installation builds on, against the real launchctl.

#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use hypercolor_macos_owner::{
    LaunchdAdapter, MacosLaunchdJob, ProcessExitWatch, parse_launchd_autostart_state,
    parse_launchd_job, wait_for_process_exit_within,
};

struct FixtureAgent {
    label: String,
    uid: u32,
    plist: tempfile::TempPath,
}

impl FixtureAgent {
    /// An agent named for `role`, running `program`.
    fn new(role: &str, program: &[&str]) -> Self {
        let uid = String::from_utf8(run("/usr/bin/id", &["-u"]).stdout)
            .expect("id prints UTF-8")
            .trim()
            .parse()
            .expect("numeric uid");
        let label = format!(
            "tech.hyperbliss.hypercolor.launchd-fixture.{role}.{}",
            std::process::id()
        );
        let program = program
            .iter()
            .map(|argument| {
                argument
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            })
            .fold(String::new(), |mut program, argument| {
                program.push_str("<string>");
                program.push_str(&argument);
                program.push_str("</string>");
                program
            });
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
<key>ProgramArguments</key><array>{program}</array>
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
    let agent = FixtureAgent::new("gate", &["/bin/sleep", "120"]);
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

    // A job with no process leaves with the bootout itself.
    let bootout = agent.launchctl(&["bootout", &target]);
    assert!(bootout.status.success(), "{bootout:?}");
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

/// launchctl answers a bootout once launchd has signalled the job, so a
/// process that takes a moment to leave keeps its job loaded past the
/// answer, and launchd drops the job when that process exits.
#[test]
fn bootout_answers_before_the_job_leaves_and_launchd_drops_it_at_exit() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let (agent, pid) = lingering_agent("lingers", scratch.path());
    let mut exit = ProcessExitWatch::arm(pid).expect("arm the exit watch");

    let target = agent.target();
    let bootout = agent.launchctl(&["bootout", &target]);
    assert!(bootout.status.success(), "{bootout:?}");
    assert_eq!(
        agent.job(),
        MacosLaunchdJob::Running { pid },
        "the job outlives launchctl's answer"
    );

    assert!(
        exit.wait_until(Instant::now() + Duration::from_secs(10))
            .expect("watch"),
        "the process leaves inside launchd's exit timeout"
    );
    assert_eq!(
        agent.job(),
        MacosLaunchdJob::NotLoaded,
        "launchd drops the job with its process"
    );
}

/// The adapter's bootout returns only once launchd has dropped the job, so
/// the same label loads again at once instead of colliding with a job
/// launchd still holds.
#[test]
fn adapter_bootout_returns_once_the_job_has_left_so_it_loads_again() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let (agent, pid) = lingering_agent("adapter", scratch.path());
    let adapter = LaunchdAdapter::with_uid(agent.uid.to_string());

    adapter
        .bootout(&adapter.target(&agent.label))
        .expect("the bootout proves the job gone");
    assert_eq!(agent.job(), MacosLaunchdJob::NotLoaded);
    assert!(
        wait_for_process_exit_within(pid, Duration::ZERO).expect("watch"),
        "the process is gone when the bootout returns"
    );
    let bootstrap = agent.bootstrap();
    assert!(bootstrap.status.success(), "{bootstrap:?}");
}

/// A running agent whose process takes a second to leave after SIGTERM,
/// with its trap installed, and that process's pid.
fn lingering_agent(role: &str, scratch: &Path) -> (FixtureAgent, u32) {
    let ready = scratch.join("trapped");
    let agent = FixtureAgent::new(
        role,
        &[
            "/bin/sh",
            "-c",
            "trap 'sleep 1; exit 0' TERM; : > \"$1\"; while :; do sleep 1; done",
            role,
            ready.to_str().expect("UTF-8 scratch path"),
        ],
    );
    let bootstrap = agent.bootstrap();
    assert!(bootstrap.status.success(), "{bootstrap:?}");
    let pid = running_pid(agent.wait_for_job(|job| matches!(job, MacosLaunchdJob::Running { .. })));
    let trapped = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            Instant::now() < trapped,
            "the agent never installed its trap"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    (agent, pid)
}

/// A process this test binary starts from inside a fixture bundle sits here.
#[test]
#[ignore = "the fixture process, started by a test below"]
fn fixture_process_waits_to_be_killed() {
    std::thread::sleep(Duration::from_mins(2));
}

#[test]
fn a_helper_in_a_bundle_reports_its_own_file_and_cdhash() {
    let root = tempfile::tempdir().expect("root");
    let root = std::fs::canonicalize(root.path()).expect("canonical root");
    let macos = root.join("Fixture.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("bundle");
    std::fs::write(
        root.join("Fixture.app/Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>fixture-app</string>
<key>CFBundleIdentifier</key><string>tech.hyperbliss.hypercolor.image-fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
"#,
    )
    .expect("Info.plist");
    let helper = macos.join("fixture-daemon");
    std::fs::copy(std::env::current_exe().expect("test binary"), &helper).expect("copy helper");

    let mut child = Command::new(&helper)
        .args(["--ignored", "--exact", "fixture_process_waits_to_be_killed"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("start helper");
    let mut inspector = hypercolor_macos_owner::NativeMacosDirectLaunchdInspector::new();
    let (executable, cdhash) = inspector
        .process_image(child.id())
        .expect("inspect helper")
        .expect("the helper runs");
    child.kill().expect("kill helper");
    child.wait().expect("reap helper");

    assert_eq!(executable, helper, "the file itself, never its bundle");
    assert_eq!(cdhash.len(), 40, "{cdhash}");
    assert!(
        inspector
            .process_image(child.id())
            .expect("inspect a process that is gone")
            .is_none()
    );
}

#[test]
fn launch_services_opens_the_bundle_at_the_path_not_its_identifier() {
    use hypercolor_macos_owner::{MacosAppHost as _, NativeMacosAppHost};

    let root = tempfile::tempdir().expect("root");
    let root = std::fs::canonicalize(root.path()).expect("canonical root");
    let marker = root.join("opened");
    let bundle = |name: &str| {
        let bundle = root.join(name).join("Fixture.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("bundle");
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>fixture</string>
<key>CFBundleIdentifier</key><string>tech.hyperbliss.hypercolor.open-fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
"#,
        )
        .expect("Info.plist");
        let executable = macos.join("fixture");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf %s \"{name}\" > \"{}\"\n",
                marker.display()
            ),
        )
        .expect("script");
        std::fs::set_permissions(
            &executable,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("mode");
        bundle
    };
    let _shadow = bundle("first");
    let exact = bundle("second");

    NativeMacosAppHost.open_bundle(&exact).expect("open");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the bundle never opened");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(std::fs::read_to_string(&marker).expect("marker"), "second");
}
