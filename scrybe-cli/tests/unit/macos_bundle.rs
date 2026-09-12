use std::cell::RefCell;

use super::*;

#[derive(Default)]
struct FakeRunner {
    calls: RefCell<Vec<Vec<String>>>,
    identities: String,
    fail_candidate_verify: bool,
}

impl FakeRunner {
    fn healthy() -> Self {
        Self {
            calls: RefCell::default(),
            identities: format!("1) HASH \"{PROJECT_SIGNING_IDENTITY}\"\n"),
            fail_candidate_verify: false,
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
}

impl CommandRunner for FakeRunner {
    fn output(&self, program: &OsStr, args: &[OsString]) -> Result<CommandOutput> {
        let mut call = vec![program.to_string_lossy().into_owned()];
        call.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
        self.calls.borrow_mut().push(call.clone());

        if call.first().is_some_and(|value| value == "security") {
            return Ok(CommandOutput {
                success: true,
                stdout: self.identities.as_bytes().to_vec(),
                stderr: Vec::new(),
            });
        }
        if call.first().is_some_and(|value| value == "codesign") {
            let verifying = call.iter().any(|value| value == "--verify");
            let candidate = call
                .last()
                .is_some_and(|value| value.contains(".scrybe-install-"));
            let success = !(verifying && candidate && self.fail_candidate_verify);
            return Ok(CommandOutput {
                success,
                stdout: Vec::new(),
                stderr: if success {
                    Vec::new()
                } else {
                    b"invalid candidate".to_vec()
                },
            });
        }
        anyhow::bail!("unexpected command: {}", call.join(" "))
    }
}

fn write_bundle(path: &Path) {
    fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    fs::create_dir_all(path.join("Contents/Resources")).unwrap();
    fs::write(
        path.join("Contents/Info.plist"),
        render_plist(env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    fs::write(path.join("Contents/MacOS/scrybe"), b"binary").unwrap();
    fs::write(
        path.join("Contents/Resources/entitlements.plist"),
        ENTITLEMENTS,
    )
    .unwrap();
}

#[test]
fn reports_missing_invalid_stale_and_ready_states() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join(BUNDLE_FILE_NAME);
    let runner = FakeRunner::healthy();

    assert_eq!(
        inspect_bundle_with(&bundle, env!("CARGO_PKG_VERSION"), &runner),
        BundleState::Missing
    );

    fs::create_dir(&bundle).unwrap();
    assert!(matches!(
        inspect_bundle_with(&bundle, env!("CARGO_PKG_VERSION"), &runner),
        BundleState::Invalid { .. }
    ));

    fs::remove_dir(&bundle).unwrap();
    write_bundle(&bundle);
    assert!(matches!(
        inspect_bundle_with(&bundle, "99.0.0", &runner),
        BundleState::Stale { .. }
    ));
    assert_eq!(
        inspect_bundle_with(&bundle, env!("CARGO_PKG_VERSION"), &runner),
        BundleState::Ready
    );
}

#[test]
fn inspection_never_executes_the_bundled_binary() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join(BUNDLE_FILE_NAME);
    write_bundle(&bundle);
    let runner = FakeRunner::healthy();

    assert_eq!(
        inspect_bundle_with(&bundle, env!("CARGO_PKG_VERSION"), &runner),
        BundleState::Ready
    );
    assert!(runner
        .calls()
        .iter()
        .all(|call| call.first().is_some_and(|program| program == "codesign")));
}

#[test]
fn identity_resolution_never_selects_an_unrelated_identity() {
    let runner = FakeRunner {
        identities: "1) HASH \"Developer ID Application: Someone Else\"\n".to_string(),
        ..FakeRunner::healthy()
    };

    let error = resolve_signing_identity_with(None, &runner).unwrap_err();

    assert!(error.to_string().contains(PROJECT_SIGNING_IDENTITY));
    assert!(!error.to_string().contains("Someone Else"));
}

#[test]
fn explicit_identity_takes_precedence_over_project_default() {
    let runner = FakeRunner {
        identities: format!("1) HASH \"{PROJECT_SIGNING_IDENTITY}\"\n2) HASH \"explicit-local\"\n"),
        ..FakeRunner::healthy()
    };

    assert_eq!(
        resolve_signing_identity_with(Some("explicit-local"), &runner).unwrap(),
        "explicit-local"
    );
}

#[test]
fn failed_candidate_verification_preserves_existing_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("source-scrybe");
    let destination = temp.path().join(BUNDLE_FILE_NAME);
    fs::write(&binary, b"new binary").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"existing bundle").unwrap();
    let runner = FakeRunner {
        fail_candidate_verify: true,
        ..FakeRunner::healthy()
    };

    let error =
        install_bundle_with(&binary, &destination, PROJECT_SIGNING_IDENTITY, &runner).unwrap_err();

    assert!(error.to_string().contains("verifying bundle candidate"));
    assert_eq!(
        fs::read(destination.join("sentinel")).unwrap(),
        b"existing bundle"
    );
    assert_eq!(
        fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains(".scrybe-install-"))
            .count(),
        0
    );
}

#[test]
fn verified_candidate_replaces_existing_bundle_after_verification() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("source-scrybe");
    let destination = temp.path().join(BUNDLE_FILE_NAME);
    fs::write(&binary, b"new binary").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"existing bundle").unwrap();
    let runner = FakeRunner::healthy();

    install_bundle_with(&binary, &destination, PROJECT_SIGNING_IDENTITY, &runner).unwrap();

    assert_eq!(
        fs::read(destination.join("Contents/MacOS/scrybe")).unwrap(),
        b"new binary"
    );
    assert!(!destination.join("sentinel").exists());
    let calls = runner.calls();
    let sign_index = calls
        .iter()
        .position(|call| call.iter().any(|arg| arg == "--force"))
        .unwrap();
    let verify_index = calls
        .iter()
        .position(|call| call.iter().any(|arg| arg == "--verify"))
        .unwrap();
    assert!(sign_index < verify_index);
}

#[test]
fn rendered_plist_has_no_unexpanded_version_marker() {
    let plist = render_plist("1.3.2");

    assert!(plist.contains("<string>1.3.2</string>"));
    assert!(!plist.contains("{{VERSION}}"));
}
