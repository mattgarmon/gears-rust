use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ENV_EXCLUDED_CRATES: &str = "DE1201_DOCS_RS_ALL_FEATURES_EXCLUDED_CRATES";
const LINT_NAME: &str = "de1201_docs_rs_all_features";

#[test]
fn cargo_lint_fixtures_cover_manifest_cases() {
    let missing_docs_rs = run_fixture("missing_docs_rs");
    assert_success("missing_docs_rs", &missing_docs_rs);
    assert_contains(
        &missing_docs_rs,
        "publishable crate `de1201_missing_docs_rs` must set `package.metadata.docs.rs.all-features = true` (DE1201)",
    );
    assert_contains(&missing_docs_rs, "`package.metadata.docs.rs` is missing");

    let env_excluded = run_fixture_with_env(
        "missing_docs_rs",
        &[(ENV_EXCLUDED_CRATES, "de1201_missing_docs_rs")],
    );
    assert_success("missing_docs_rs env exclusion", &env_excluded);
    assert_not_contains(&env_excluded, "DE1201");

    for fixture in ["all_features_true", "publish_false", "excluded_crate"] {
        let output = run_fixture(fixture);
        assert_success(fixture, &output);
        assert_not_contains(&output, "DE1201");
    }
}

fn run_fixture(name: &str) -> Output {
    run_fixture_with_env(name, &[])
}

fn run_fixture_with_env(name: &str, extra_env: &[(&str, &str)]) -> Output {
    let fixture = fixtures_dir().join(name);
    let manifest_path = fixture.join("Cargo.toml");
    let lint_parent_dir = lint_parent_dir();
    let target_dir = fixture
        .join("target")
        .join(target_dir_name(name, extra_env));

    let mut command = Command::new("cargo");
    command
        .arg("dylint")
        .arg("--path")
        .arg(&lint_parent_dir)
        .arg("--pattern")
        .arg(LINT_NAME)
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--no-deps")
        .env("CARGO_TARGET_DIR", target_dir)
        .env_remove(ENV_EXCLUDED_CRATES)
        .env_remove("DYLINT_RUSTFLAGS")
        .env_remove("DYLINT_TOML")
        .env_remove("RUSTFLAGS")
        .current_dir(&fixture);

    for (key, value) in extra_env {
        command.env(key, value);
    }

    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run cargo dylint for fixture `{name}`: {error}"));

    remove_fixture_lockfile(&fixture);
    output
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn lint_parent_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("lint crate should have a parent directory")
        .to_path_buf()
}

fn remove_fixture_lockfile(fixture: &Path) {
    match fs::remove_file(fixture.join("Cargo.lock")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!(
            "failed to remove fixture Cargo.lock in `{}`: {error}",
            fixture.display()
        ),
    }
}

fn target_dir_name(name: &str, extra_env: &[(&str, &str)]) -> String {
    if extra_env.is_empty() {
        return format!("dylint-fixture-{name}");
    }

    let env_suffix = extra_env
        .iter()
        .map(|(key, value)| format!("{key}-{value}"))
        .collect::<Vec<_>>()
        .join("-");

    format!("dylint-fixture-{name}-{env_suffix}")
}

fn assert_success(name: &str, output: &Output) {
    if output.status.success() {
        return;
    }

    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_else(|_| "<unset>".into());
    let has_dylint_link = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join("dylint-link").exists()))
        .unwrap_or(false);

    panic!(
        "fixture `{name}` failed (exit code: {:?})\n\
         --- diagnostics ---\n\
         RUSTUP_TOOLCHAIN={toolchain}\n\
         dylint-link in PATH: {has_dylint_link}\n\
         --- stdout ---\n{}\n\
         --- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_contains(output: &Output, needle: &str) {
    let combined = combined_output(output);
    assert!(
        combined.contains(needle),
        "expected output to contain `{needle}`\noutput:\n{combined}"
    );
}

fn assert_not_contains(output: &Output, needle: &str) {
    let combined = combined_output(output);
    assert!(
        !combined.contains(needle),
        "expected output not to contain `{needle}`\noutput:\n{combined}"
    );
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
