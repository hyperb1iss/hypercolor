use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    BootstrapCommand, EXIT_NO_ACTIVATOR, EXIT_NO_PIPE, EXIT_NO_RELEASE, UpdateServiceConfig,
    UpdateServiceExit, UpdateServiceHost, UpdateServiceMode, fallback_command, install_bootstrap,
    recover_fallback, restore_bootstrap, run_update_service, session_logon_notice,
    update_service_sddl,
};
use crate::windows::machine_layout::BootstrapSlot;

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn a_start_names_prepare_or_activate_and_no_argument_means_recover() {
    for (values, mode) in [
        (&[][..], Some(UpdateServiceMode::Recover)),
        (&["prepare"][..], Some(UpdateServiceMode::Prepare)),
        (&["activate"][..], Some(UpdateServiceMode::Activate)),
        (&["recover"][..], None),
        (&["activate", "again"][..], None),
        (&["ACTIVATE"][..], None),
    ] {
        assert_eq!(
            UpdateServiceMode::parse(&arguments(values)),
            mode,
            "{values:?}"
        );
    }
}

#[test]
fn each_mode_runs_through_the_units_cli() {
    assert_eq!(
        UpdateServiceMode::Recover.activator_arguments(),
        ["update", "activator", "recover"]
    );
    assert_eq!(
        UpdateServiceMode::Activate.activator_arguments(),
        ["update", "activator", "activate"]
    );
    assert_eq!(
        UpdateServiceMode::Prepare.activator_arguments(),
        ["update", "activator", "prepare"]
    );
}

#[test]
fn only_a_zero_exit_stops_without_recovery() {
    assert_eq!(
        UpdateServiceExit::of_activator(0),
        UpdateServiceExit::Stopped
    );
    assert_eq!(
        UpdateServiceExit::of_activator(1),
        UpdateServiceExit::Failed(1)
    );
    assert_eq!(
        UpdateServiceExit::of_activator(0xC000_0005_u32.cast_signed()),
        UpdateServiceExit::Failed(0xC000_0005),
        "a crash keeps its NTSTATUS"
    );
}

#[test]
fn a_session_logon_is_one_line_for_the_activator() {
    assert_eq!(session_logon_notice(3), "session-logon 3\n");
}

/// A host that records what the service asked of it, in order.
struct FakeHost {
    steps: Vec<String>,
    selection: Result<&'static str, ()>,
    pipe: bool,
    activator: io::Result<i32>,
}

impl FakeHost {
    fn working(code: i32) -> Self {
        Self {
            steps: Vec::new(),
            selection: Ok("prior"),
            pipe: true,
            activator: Ok(code),
        }
    }
}

impl UpdateServiceHost for FakeHost {
    type Pipe = &'static str;
    type Activator = &'static str;
    type Error = ();

    fn select_activator(&mut self) -> Result<Self::Activator, ()> {
        self.steps.push("select".to_owned());
        self.selection
    }

    fn open_pipe(&mut self) -> io::Result<Self::Pipe> {
        self.steps.push("pipe".to_owned());
        if self.pipe {
            Ok("pipe")
        } else {
            Err(io::ErrorKind::AddrInUse.into())
        }
    }

    fn run_activator(
        &mut self,
        activator: Self::Activator,
        mode: UpdateServiceMode,
        pipe: Self::Pipe,
    ) -> io::Result<i32> {
        self.steps.push(format!("run {activator} {mode:?} {pipe}"));
        std::mem::replace(&mut self.activator, Ok(0))
    }
}

#[test]
fn the_pipe_exists_before_the_activator_runs() {
    let mut host = FakeHost::working(0);

    let exit = run_update_service(&mut host, &arguments(&["activate"]));

    assert_eq!(exit, UpdateServiceExit::Stopped);
    assert_eq!(host.steps, ["select", "pipe", "run prior Activate pipe"]);
}

#[test]
fn a_failed_activator_fails_the_run_with_its_code() {
    let mut host = FakeHost::working(73);

    assert_eq!(
        run_update_service(&mut host, &[]),
        UpdateServiceExit::Failed(73)
    );
    assert_eq!(host.steps[2], "run prior Recover pipe");
}

#[test]
fn arguments_no_mode_names_stop_quietly_and_touch_nothing() {
    let mut host = FakeHost::working(1);

    assert_eq!(
        run_update_service(&mut host, &arguments(&["--anything"])),
        UpdateServiceExit::Stopped
    );
    assert!(host.steps.is_empty());
}

#[test]
fn a_run_that_cannot_proceed_says_where_it_stopped() {
    let mut host = FakeHost::working(0);
    host.selection = Err(());
    assert_eq!(
        run_update_service(&mut host, &[]),
        UpdateServiceExit::Failed(EXIT_NO_RELEASE)
    );
    assert_eq!(host.steps, ["select"], "nothing runs without a release");

    let mut host = FakeHost::working(0);
    host.pipe = false;
    assert_eq!(
        run_update_service(&mut host, &[]),
        UpdateServiceExit::Failed(EXIT_NO_PIPE)
    );
    assert_eq!(
        host.steps,
        ["select", "pipe"],
        "nothing runs without a pipe"
    );

    let mut host = FakeHost::working(0);
    host.activator = Err(io::ErrorKind::NotFound.into());
    assert_eq!(
        run_update_service(&mut host, &[]),
        UpdateServiceExit::Failed(EXIT_NO_ACTIVATOR)
    );
}

#[test]
fn the_fallback_command_quotes_a_path_with_spaces() {
    assert_eq!(
        fallback_command(Path::new("/Program Files/Hypercolor/update/slot-b.exe")),
        OsString::from("\"/Program Files/Hypercolor/update/slot-b.exe\" --recover-fallback")
    );
}

#[test]
fn a_bootstrap_process_does_one_of_four_things() {
    for (values, command) in [
        (&[][..], Some(BootstrapCommand::Service)),
        (
            &["--recover-fallback"][..],
            Some(BootstrapCommand::RecoverFallback),
        ),
        (
            &["--register-service"][..],
            Some(BootstrapCommand::RegisterService),
        ),
        (&["--probe"][..], Some(BootstrapCommand::Probe)),
        (&["--recover-fallback", "extra"][..], None),
        (&["recover"][..], None),
    ] {
        assert_eq!(
            BootstrapCommand::parse(&arguments(values)),
            command,
            "{values:?}"
        );
    }
}

#[test]
fn the_recorded_user_may_start_and_watch_the_service_and_nothing_more() {
    let sid = "S-1-5-21-1004336348-1177238915-682003330-1001";

    let sddl = update_service_sddl(sid);

    assert!(sddl.ends_with(&format!("(A;;LCRP;;;{sid})")), "{sddl}");
    assert_eq!(
        sddl.matches(";;;").count(),
        3,
        "SYSTEM, Administrators and the recorded user: {sddl}"
    );
}

/// A registration that records its state, and fails on a step. Its image
/// and failure action live in a registry that reaches the disk only when
/// flushed; its slots are files, on disk as they are written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeRegistration {
    image: BootstrapSlot,
    fallback: BootstrapSlot,
    /// The image and failure action as the last flush wrote them.
    on_disk: (BootstrapSlot, BootstrapSlot),
    slots: [&'static str; 2],
    written: Vec<(BootstrapSlot, PathBuf)>,
    started: u32,
    steps: Vec<&'static str>,
    crash_at: Option<&'static str>,
}

impl FakeRegistration {
    fn installed() -> Self {
        Self {
            image: BootstrapSlot::A,
            fallback: BootstrapSlot::B,
            on_disk: (BootstrapSlot::A, BootstrapSlot::B),
            slots: ["proven", "retained"],
            written: Vec::new(),
            started: 0,
            steps: Vec::new(),
            crash_at: None,
        }
    }

    /// Lose power: what the registry never flushed is gone.
    fn power_cut(&mut self) {
        (self.image, self.fallback) = self.on_disk;
    }

    fn crash(&mut self, step: &'static str) -> io::Result<()> {
        self.steps.push(step);
        if self.crash_at == Some(step) {
            self.crash_at = None;
            return Err(io::Error::other("killed"));
        }
        Ok(())
    }

    fn slot_contents(&self, slot: BootstrapSlot) -> &'static str {
        self.slots[match slot {
            BootstrapSlot::A => 0,
            BootstrapSlot::B => 1,
        }]
    }
}

impl UpdateServiceConfig for FakeRegistration {
    fn set_image(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        self.crash("image")?;
        self.image = slot;
        Ok(())
    }

    fn set_fallback(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        self.crash("fallback")?;
        self.fallback = slot;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.crash("flush")?;
        self.on_disk = (self.image, self.fallback);
        Ok(())
    }

    fn write_slot(&mut self, slot: BootstrapSlot, source: &Path) -> io::Result<()> {
        self.crash("write")?;
        self.written.push((slot, source.to_path_buf()));
        self.slots[match slot {
            BootstrapSlot::A => 0,
            BootstrapSlot::B => 1,
        }] = "new";
        Ok(())
    }

    fn probe_slot(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        self.crash("probe")?;
        if self.slot_contents(slot) == "broken" {
            return Err(io::Error::other("would not load"));
        }
        Ok(())
    }

    fn start(&mut self) -> io::Result<()> {
        self.crash("start")?;
        self.started += 1;
        Ok(())
    }
}

#[test]
fn the_fallback_takes_the_service_back_to_its_slot_before_starting_it() {
    let mut service = FakeRegistration::installed();

    recover_fallback(&mut service, BootstrapSlot::B).expect("fall back");

    assert_eq!(service.steps, ["image", "fallback", "flush", "start"]);
    assert_eq!(
        service.on_disk,
        (BootstrapSlot::B, BootstrapSlot::B),
        "the recovery the start runs never journals under an image a power cut takes back"
    );
    assert_eq!(service.image, BootstrapSlot::B);
    assert_eq!(service.fallback, BootstrapSlot::B);
    assert_eq!(service.started, 1);
}

#[test]
fn a_refused_switch_does_not_start_the_broken_image() {
    let mut service = FakeRegistration::installed();
    service.crash_at = Some("image");

    recover_fallback(&mut service, BootstrapSlot::B).expect_err("refused");

    assert_eq!(service.started, 0);
}

#[test]
fn a_new_bootstrap_lands_in_the_inactive_slot_behind_the_proven_one() {
    let mut service = FakeRegistration::installed();

    install_bootstrap(
        &mut service,
        BootstrapSlot::A,
        Path::new("unit/bootstrap.exe"),
    )
    .expect("install");

    assert_eq!(
        service.steps,
        ["fallback", "flush", "write", "probe", "image"]
    );
    assert_eq!(service.image, BootstrapSlot::B, "the new bootstrap runs");
    assert_eq!(
        service.fallback,
        BootstrapSlot::A,
        "the proven bootstrap is retained"
    );
    assert_eq!(service.slot_contents(BootstrapSlot::A), "proven");
    assert_eq!(service.slot_contents(BootstrapSlot::B), "new");
    assert_eq!(service.started, 0, "a transaction starts nothing here");
}

#[test]
fn at_every_crash_point_the_failure_action_names_a_slot_with_the_proven_bootstrap() {
    for step in ["fallback", "flush", "write", "probe", "image"] {
        let mut service = FakeRegistration::installed();
        service.crash_at = Some(step);

        install_bootstrap(&mut service, BootstrapSlot::A, Path::new("new.exe"))
            .expect_err("crashed");

        assert_eq!(
            service.slot_contents(BootstrapSlot::A),
            "proven",
            "crash at {step}"
        );
        if step == "fallback" {
            assert_eq!(service.fallback, BootstrapSlot::B, "not yet moved");
            assert_eq!(service.image, BootstrapSlot::A);
        } else {
            assert_eq!(service.fallback, BootstrapSlot::A, "crash at {step}");
        }

        install_bootstrap(&mut service, BootstrapSlot::A, Path::new("new.exe"))
            .expect("resume after the crash");
        assert_eq!(service.image, BootstrapSlot::B, "crash at {step}");
        assert_eq!(service.fallback, BootstrapSlot::A, "crash at {step}");
    }
}

#[test]
fn no_power_cut_leaves_the_failure_action_on_the_slot_being_written() {
    for step in ["fallback", "write", "probe", "image"] {
        let mut service = FakeRegistration::installed();
        service.crash_at = Some(step);
        install_bootstrap(&mut service, BootstrapSlot::A, Path::new("new.exe"))
            .expect_err("crashed");

        service.power_cut();

        assert_ne!(
            service.slot_contents(service.fallback),
            "new",
            "power cut at {step}: the failure action names an unproven bootstrap"
        );
        assert_eq!(
            service.slot_contents(service.image),
            "proven",
            "power cut at {step}"
        );
    }
}

#[test]
fn a_bootstrap_that_does_not_load_never_becomes_the_image() {
    struct Unloadable(FakeRegistration);

    impl UpdateServiceConfig for Unloadable {
        fn set_image(&mut self, slot: BootstrapSlot) -> io::Result<()> {
            self.0.set_image(slot)
        }
        fn set_fallback(&mut self, slot: BootstrapSlot) -> io::Result<()> {
            self.0.set_fallback(slot)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
        fn write_slot(&mut self, slot: BootstrapSlot, source: &Path) -> io::Result<()> {
            self.0.write_slot(slot, source)?;
            self.0.slots[1] = "broken";
            Ok(())
        }
        fn probe_slot(&mut self, slot: BootstrapSlot) -> io::Result<()> {
            self.0.probe_slot(slot)
        }
        fn start(&mut self) -> io::Result<()> {
            self.0.start()
        }
    }
    let mut service = Unloadable(FakeRegistration::installed());

    install_bootstrap(&mut service, BootstrapSlot::A, Path::new("broken.exe"))
        .expect_err("a bootstrap that will not load is refused");

    assert_eq!(service.0.image, BootstrapSlot::A);
    assert!(!service.0.steps.contains(&"image"), "{:?}", service.0.steps);
}

#[test]
fn compensation_returns_to_the_slot_the_failure_action_already_names() {
    let mut service = FakeRegistration::installed();
    install_bootstrap(&mut service, BootstrapSlot::A, Path::new("new.exe")).expect("install");
    service.steps.clear();

    restore_bootstrap(&mut service, BootstrapSlot::A).expect("restore");

    assert_eq!(service.steps, ["image"], "one step, one durable change");
    assert_eq!(service.image, BootstrapSlot::A);
    assert_eq!(service.fallback, BootstrapSlot::A);
}

#[test]
fn compensation_after_a_crash_before_the_switch_changes_nothing_that_matters() {
    let mut service = FakeRegistration::installed();
    service.crash_at = Some("write");
    install_bootstrap(&mut service, BootstrapSlot::A, Path::new("new.exe")).expect_err("crashed");

    restore_bootstrap(&mut service, BootstrapSlot::A).expect("restore");

    assert_eq!(service.image, BootstrapSlot::A);
    assert_eq!(service.fallback, BootstrapSlot::A);
    assert_eq!(service.slot_contents(BootstrapSlot::A), "proven");
}
