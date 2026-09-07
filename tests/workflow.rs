use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use filetrail::config::Config;
use filetrail::config::Entry;
use filetrail::config::Store;
use git2::Repository;
use tempfile::TempDir;

struct CompletionFixture {
    temp: TempDir,
    home: PathBuf,
    binary: PathBuf,
}

impl CompletionFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let bin = temp.path().join("bin 'quoted' $cash \\files");
        fs::create_dir(&bin).unwrap();
        let binary = bin.join("filetrail");
        fs::copy(env!("CARGO_BIN_EXE_filetrail"), &binary).unwrap();
        Self { temp, home, binary }
    }

    fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        // Fish only autoloads completions for commands it can resolve. Model an
        // installed binary instead of depending on FileTrail in the user's PATH.
        let mut paths = vec![self.binary.parent().unwrap().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = Command::new(executable);
        command
            .current_dir(&self.home)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("TERM", "xterm")
            .env_remove("BASH_ENV")
            .env_remove("ENV");
        command
    }

    fn install(&self, shell: &str) -> String {
        output_text(
            self.command(&self.binary)
                .env("SHELL", format!("/bin/{shell}"))
                .args(["completions", "--install"])
                .output()
                .unwrap(),
        )
    }
}

fn output_text(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn completion_generation_includes_nested_commands_without_initialization() {
    let f = CompletionFixture::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let script = output_text(
            f.command(&f.binary)
                .args(["completions", shell])
                .output()
                .unwrap(),
        );
        for expected in [
            "daemon",
            "restart",
            "service",
            "uninstall",
            "from",
            "install",
        ] {
            assert!(script.contains(expected), "{shell}: missing {expected}");
        }
    }
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}

#[test]
fn completion_installation_is_idempotent_and_preserves_existing_profiles() {
    let f = CompletionFixture::new();
    let existing = "# user configuration\nexport FILETRAIL_TEST=preserved";
    for name in [
        ".bashrc",
        ".bash_login",
        ".zshrc",
        ".config/fish/completions/filetrail.fish",
    ] {
        let path = f.home.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, existing).unwrap();
    }
    for (shell, names) in [
        ("bash", vec![".bashrc", ".bash_login"]),
        ("zsh", vec![".zshrc"]),
        ("fish", vec![".config/fish/completions/filetrail.fish"]),
    ] {
        assert!(f.install(shell).contains("Open a new shell"));
        let first: Vec<_> = names
            .iter()
            .map(|name| fs::read(f.home.join(name)).unwrap())
            .collect();
        f.install(shell);
        for (name, first) in names.iter().zip(first) {
            let path = f.home.join(name);
            assert_eq!(fs::read(&path).unwrap(), first);
            let content = fs::read_to_string(path).unwrap();
            assert!(content.starts_with(existing));
            assert_eq!(
                content.matches("# >>> filetrail completions >>>").count(),
                1
            );
        }
    }
    assert!(!f.home.join(".bash_profile").exists());
    assert!(!f.home.join(".filetrail").exists());
}

#[test]
fn completion_installation_respects_overrides_symlinks_and_permissions() {
    let f = CompletionFixture::new();
    let dotdir = f.home.join("zsh config");
    fs::create_dir(&dotdir).unwrap();
    let target = f.home.join("tracked-zshrc");
    fs::write(&target, "# tracked dotfile\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, dotdir.join(".zshrc")).unwrap();
    output_text(
        f.command(&f.binary)
            .env("ZDOTDIR", &dotdir)
            .args(["completions", "zsh", "--install"])
            .output()
            .unwrap(),
    );
    assert!(
        fs::symlink_metadata(dotdir.join(".zshrc"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(
        fs::read_to_string(target)
            .unwrap()
            .starts_with("# tracked dotfile\n")
    );
    let xdg = f.home.join("fish config");
    output_text(
        f.command(&f.binary)
            .env("XDG_CONFIG_HOME", &xdg)
            .args(["completions", "fish", "--install"])
            .output()
            .unwrap(),
    );
    assert!(xdg.join("fish/completions/filetrail.fish").is_file());
    assert!(!f.home.join(".zshrc").exists());
    assert!(!f.home.join(".config").exists());
}

#[test]
fn completion_installation_refuses_invalid_input_before_changing_profiles() {
    let f = CompletionFixture::new();
    fs::write(f.home.join(".bashrc"), "# keep me\n").unwrap();
    fs::write(
        f.home.join(".bash_profile"),
        "# >>> filetrail completions >>>\n",
    )
    .unwrap();
    for args in [
        vec!["completions", "bash", "--install"],
        vec!["completions", "elvish", "--install"],
        vec!["completions", "--install"],
    ] {
        let output = f
            .command(&f.binary)
            .env_remove("SHELL")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    assert_eq!(
        fs::read_to_string(f.home.join(".bashrc")).unwrap(),
        "# keep me\n"
    );
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 2);
}

#[test]
fn bash_completion_loads_and_completes_commands_options_and_paths() {
    let f = CompletionFixture::new();
    f.install("bash");
    fs::write(f.home.join("example.txt"), "").unwrap();
    for (words, index, expected) in [
        ("filetrail co", "1", "commit"),
        ("filetrail daemon st", "2", "start"),
        ("filetrail service un", "2", "uninstall"),
        ("filetrail add --f", "2", "--from"),
        ("filetrail add --from ex", "3", "example.txt"),
    ] {
        let script = format!(
            "complete -p filetrail >/dev/null || exit 1; COMP_WORDS=({words}); COMP_CWORD={index}; _filetrail filetrail \"${{COMP_WORDS[COMP_CWORD]}}\" \"${{COMP_WORDS[COMP_CWORD-1]}}\"; printf '%s\\n' \"${{COMPREPLY[@]}}\""
        );
        let output = output_text(
            f.command("bash")
                .args(["--noprofile", "-ic", &script])
                .output()
                .unwrap(),
        );
        assert!(
            output.lines().any(|line| line == expected),
            "{words}: {output}"
        );
    }
}

#[test]
fn zsh_completion_registers_with_and_without_existing_compinit() {
    for insecure in [false, true] {
        for prefix in ["", "autoload -Uz compinit\ncompinit -i\n"] {
            let f = CompletionFixture::new();
            let functions = f.temp.path().join("completion functions");
            fs::create_dir(&functions).unwrap();
            fs::write(
                functions.join("_filetrail_fixture"),
                "#compdef filetrail-fixture\n",
            )
            .unwrap();
            fs::set_permissions(
                &functions,
                fs::Permissions::from_mode(if insecure { 0o777 } else { 0o755 }),
            )
            .unwrap();
            fs::write(
                f.home.join(".zshrc"),
                format!("fpath=(\"$FILETRAIL_TEST_FPATH\" $fpath)\n{prefix}"),
            )
            .unwrap();
            f.install("zsh");
            // No TTY: an audit prompt must not abort completion initialization.
            // Safe fixture completions should load; unsafe ones must be ignored.
            let output = f.command("zsh")
                .env("FILETRAIL_TEST_FPATH", &functions)
                .env("FILETRAIL_TEST_COMPLETION", if insecure { "" } else { "_filetrail_fixture" })
                .args([
                    "-d",
                    "-ic",
                    "[[ ${_comps[filetrail]-} == _filetrail && ${_comps[filetrail-fixture]-} == $FILETRAIL_TEST_COMPLETION ]] && (( $+functions[_filetrail] ))",
                ])
                .output()
                .unwrap();
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output_text(output);
        }
    }
}

#[test]
fn fish_completion_autoloads_commands_options_and_paths() {
    let f = CompletionFixture::new();
    f.install("fish");
    let resolved = output_text(
        f.command("fish")
            .args(["-c", "command -s filetrail"])
            .output()
            .unwrap(),
    );
    assert_eq!(resolved.trim_end(), f.binary.to_str().unwrap());
    fs::write(f.home.join("example.txt"), "").unwrap();
    for (line, expected) in [
        ("filetrail co", "commit"),
        ("filetrail daemon st", "start"),
        ("filetrail service un", "uninstall"),
        ("filetrail add --f", "--from"),
        ("filetrail add --from ex", "example.txt"),
    ] {
        let script = format!("complete -C '{line}'");
        let result = f.command("fish").args(["-c", &script]).output().unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        let output = output_text(result);
        assert!(
            output
                .lines()
                .any(|line| line.split('\t').next() == Some(expected)),
            "{line}: {output}\nstderr: {stderr}"
        );
    }
}

#[test]
fn install_wrapper_uses_cargo_then_installs_completion_only_on_success() {
    let f = CompletionFixture::new();
    let fake_bin = f.temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let cargo = fake_bin.join("cargo");
    fs::write(&cargo, "#!/bin/sh\nexit 19\n").unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let root = f.temp.path().join("install root");
    let run = || {
        f.command("sh")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
            .arg("zsh")
            .env("CARGO_INSTALL_ROOT", &root)
            .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
            .env("FILETRAIL_TEST_BINARY", &f.binary)
            .output()
            .unwrap()
    };
    assert_eq!(run().status.code(), Some(19));
    assert!(!f.home.join(".zshrc").exists());
    fs::write(&cargo, "#!/bin/sh\nset -eu\n[ \"$1 $2 $3 $4 $5\" = 'install --path . --locked --root' ]\nmkdir -p \"$6/bin\"\ncp \"$FILETRAIL_TEST_BINARY\" \"$6/bin/filetrail\"\n").unwrap();
    output_text(run());
    assert!(root.join("bin/filetrail").is_file());
    assert!(
        fs::read_to_string(f.home.join(".zshrc"))
            .unwrap()
            .contains(&format!("{}/bin/filetrail", root.display()))
    );
}

struct Fixture {
    _temp: TempDir,
    store: Store,
    source: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new(subdir: &str, delete: bool) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("ft-")
            .tempdir_in("/tmp")
            .unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let repository = temp.path().join("repo");
        let repo = Repository::init(&repository).unwrap();
        let mut gitconfig = repo.config().unwrap();
        gitconfig.set_str("user.name", "Filetrail Test").unwrap();
        gitconfig
            .set_str("user.email", "filetrail@example.invalid")
            .unwrap();
        let store = Store::new(temp.path().join("state")).unwrap();
        let mut config =
            Config::new(fs::canonicalize(&repository).unwrap(), subdir.into()).unwrap();
        config.scan_interval_secs = 1;
        config.entries.push(Entry {
            id: 1,
            source: fs::canonicalize(&source).unwrap(),
            target: "config".into(),
            directory: true,
            enabled: true,
            delete,
            exclude: vec![],
        });
        store.save_config(&config).unwrap();
        Self {
            _temp: temp,
            store,
            source,
            repository,
        }
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.source.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn target(&self, name: &str) -> PathBuf {
        let config = self.store.config().unwrap();
        self.repository
            .join(&config.subdir)
            .join("config")
            .join(name)
    }

    fn sync(&self) {
        let report = filetrail::sync::run(&self.store, false, None).unwrap();
        assert!(report.errors.is_empty(), "{}", report.text());
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_filetrail"))
            .arg("--data-dir")
            .arg(&self.store.root)
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn platform_subdir_recursive_sync_and_idempotence() {
    let f = Fixture::new("macos", false);
    f.write("nvim/init.lua", "return {}\n");
    f.sync();
    assert_eq!(
        fs::read_to_string(f.target("nvim/init.lua")).unwrap(),
        "return {}\n"
    );
    assert!(!f.repository.join("config").exists());
    assert!(
        filetrail::sync::run(&f.store, false, None)
            .unwrap()
            .actions
            .is_empty()
    );
    f.write("nvim/init.lua", "return { updated = true }\n");
    f.sync();
    assert!(
        fs::read_to_string(f.target("nvim/init.lua"))
            .unwrap()
            .contains("updated")
    );
}

#[test]
fn dry_run_does_not_write_files_or_state() {
    let f = Fixture::new("", false);
    f.write("a", "one");
    let report = filetrail::sync::run(&f.store, true, None).unwrap();
    assert!(report.actions.iter().any(|line| line == "add config/a"));
    assert!(!f.target("a").exists());
    assert!(!f.store.root.join("state.db").exists());
}

#[test]
fn first_sync_and_external_edits_are_protected_and_resolvable() {
    let f = Fixture::new("linux", false);
    f.write("a", "source");
    fs::create_dir_all(f.target("")).unwrap();
    fs::write(f.target("a"), "existing").unwrap();
    let report = filetrail::sync::run(&f.store, false, None).unwrap();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(fs::read_to_string(f.target("a")).unwrap(), "existing");
    assert!(
        filetrail::sync::run(&f.store, false, Some(Path::new("linux/config/a")))
            .unwrap()
            .errors
            .is_empty()
    );
    fs::write(f.target("a"), "manual edit").unwrap();
    f.write("a", "new source");
    assert_eq!(
        filetrail::sync::run(&f.store, false, None)
            .unwrap()
            .errors
            .len(),
        1
    );
    assert_eq!(fs::read_to_string(f.target("a")).unwrap(), "manual edit");
}

#[test]
fn deletion_is_opt_in_and_never_deletes_unowned_files() {
    for delete in [false, true] {
        let f = Fixture::new("", delete);
        f.write("a", "one");
        f.sync();
        fs::write(f.target("manual"), "keep").unwrap();
        fs::remove_file(f.source.join("a")).unwrap();
        f.sync();
        assert_eq!(f.target("a").exists(), !delete);
        assert!(f.target("manual").exists());
    }
}

#[test]
fn missing_source_directory_never_causes_mass_deletion() {
    let f = Fixture::new("", true);
    f.write("a", "one");
    f.sync();
    fs::rename(&f.source, f.source.with_extension("offline")).unwrap();
    assert!(
        !filetrail::sync::run(&f.store, false, None)
            .unwrap()
            .errors
            .is_empty()
    );
    assert!(f.target("a").exists());
}

#[test]
fn ignored_files_and_nested_git_are_skipped() {
    let f = Fixture::new("", true);
    let mut config = f.store.config().unwrap();
    config.exclude = vec!["**/*.tmp".into(), "cache".into()];
    f.store.save_config(&config).unwrap();
    f.write("keep", "yes");
    f.write("a.tmp", "no");
    f.write("cache/data", "no");
    f.write("nested/.git/config", "no");
    f.sync();
    assert!(f.target("keep").exists());
    assert!(!f.target("a.tmp").exists());
    assert!(!f.target("cache/data").exists());
    assert!(!f.target("nested/.git/config").exists());
}

#[test]
fn newly_excluded_files_are_not_deleted() {
    let f = Fixture::new("", true);
    f.write("cache/a", "keep");
    f.sync();
    let mut config = f.store.config().unwrap();
    config.exclude = vec!["cache".into()];
    f.store.save_config(&config).unwrap();
    f.sync();
    assert!(f.target("cache/a").exists());
}

#[test]
fn symlinks_and_executable_permissions_are_preserved() {
    let f = Fixture::new("", false);
    f.write("script", "#!/bin/sh\n");
    fs::set_permissions(f.source.join("script"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("missing", f.source.join("link")).unwrap();
    f.sync();
    assert_eq!(
        fs::read_link(f.target("link")).unwrap(),
        PathBuf::from("missing")
    );
    assert_eq!(
        fs::metadata(f.target("script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn destination_symlink_ancestors_cannot_escape_repository() {
    let f = Fixture::new("", false);
    f.write("a", "one");
    let outside = f._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.repository.join("config")).unwrap();
    assert!(
        !filetrail::sync::run(&f.store, false, None)
            .unwrap()
            .errors
            .is_empty()
    );
    assert!(!outside.join("a").exists());
}

#[test]
fn default_commit_message_and_untracked_diff_include_files() {
    let f = Fixture::new("macos", true);
    f.write("a", "hello\n");
    f.sync();
    fs::write(f.repository.join("unrelated"), "do not commit").unwrap();
    let diff = filetrail::git::diff(&f.store, &[]).unwrap();
    assert!(diff.contains("+hello"), "{diff}");
    filetrail::git::commit(&f.store, None, &[]).unwrap();
    let repo = Repository::open(&f.repository).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert!(commit.message().unwrap().starts_with("filetrail: "));
    assert!(commit.message().unwrap().contains("macos/config/a"));
    assert!(
        commit
            .tree()
            .unwrap()
            .get_path(Path::new("unrelated"))
            .is_err()
    );
    f.write("a", "updated\n");
    f.write("b", "new\n");
    f.sync();
    filetrail::git::commit(&f.store, Some("custom message"), &["macos/config/a".into()]).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.message().unwrap(), "custom message");
    assert!(
        commit
            .tree()
            .unwrap()
            .get_path(Path::new("macos/config/b"))
            .is_err()
    );
    fs::remove_file(f.source.join("a")).unwrap();
    f.sync();
    filetrail::git::commit(&f.store, None, &[]).unwrap();
    assert!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .contains("delete \"macos/config/a\"")
    );
}

#[test]
fn preexisting_staging_is_not_modified() {
    let f = Fixture::new("", false);
    f.write("a", "hello");
    f.sync();
    fs::write(f.repository.join("unrelated"), "manual").unwrap();
    let repo = Repository::open(&f.repository).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("unrelated")).unwrap();
    index.write().unwrap();
    let before = fs::read(repo.path().join("index")).unwrap();
    assert!(filetrail::git::commit(&f.store, None, &[]).is_err());
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    assert!(repo.head().is_err());
}

#[test]
fn unresolved_git_operation_blocks_sync_and_commit() {
    let f = Fixture::new("", false);
    f.write("a", "hello");
    fs::write(
        f.repository.join(".git/MERGE_HEAD"),
        "0000000000000000000000000000000000000000\n",
    )
    .unwrap();
    assert!(filetrail::sync::run(&f.store, false, None).is_err());
    assert!(!f.target("a").exists());
    assert!(filetrail::git::commit(&f.store, None, &[]).is_err());
}

#[test]
fn invalid_configuration_does_not_replace_existing_config() {
    let f = Fixture::new("", false);
    let mut config = f.store.config().unwrap();
    let before = fs::read(f.store.root.join("config.toml")).unwrap();
    let mut duplicate = config.entries[0].clone();
    duplicate.id = 2;
    config.entries.push(duplicate);
    assert!(f.store.save_config(&config).is_err());
    assert_eq!(fs::read(f.store.root.join("config.toml")).unwrap(), before);
}

#[test]
fn corrupt_state_is_not_silently_reset() {
    let f = Fixture::new("", false);
    f.write("a", "hello");
    fs::write(f.store.root.join("state.db"), "broken").unwrap();
    assert!(filetrail::sync::run(&f.store, false, None).is_err());
    assert!(!f.target("a").exists());
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_watches_atomic_saves_pause_resume_and_config_reload() {
    let f = Fixture::new("", false);
    let mut config = f.store.config().unwrap();
    config.scan_interval_secs = 3600;
    f.store.save_config(&config).unwrap();
    f.write("a", "initial");
    let child = Command::new(env!("CARGO_BIN_EXE_filetrail"))
        .arg("--data-dir")
        .arg(&f.store.root)
        .args(["daemon", "run"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    wait_until(|| f.target("a").exists());
    f.write("a", "native event");
    wait_until(|| fs::read_to_string(f.target("a")).is_ok_and(|content| content == "native event"));
    let paused = f.cli(&["pause"]);
    assert!(
        paused.status.success(),
        "{}",
        String::from_utf8_lossy(&paused.stderr)
    );
    f.write("replacement", "replacement");
    fs::rename(f.source.join("replacement"), f.source.join("a")).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(fs::read_to_string(f.target("a")).unwrap(), "native event");
    assert!(f.cli(&["resume"]).status.success());
    wait_until(|| fs::read_to_string(f.target("a")).is_ok_and(|content| content == "replacement"));
    assert!(f.cli(&["disable", "1"]).status.success());
    f.write("b", "later");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!f.target("b").exists());
    assert!(f.cli(&["enable", "1"]).status.success());
    wait_until(|| f.target("b").exists());
    assert!(f.cli(&["daemon", "stop"]).status.success());
    assert!(child.0.wait().unwrap().success());
    assert!(!f.store.root.join("daemon.sock").exists());
}

#[test]
fn cli_init_subdir_and_list_import() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("state");
    let repo = temp.path().join("repo");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_filetrail"))
            .arg("--data-dir")
            .arg(&config)
            .args(args)
            .output()
            .unwrap()
    };
    let init = run(&["init", repo.to_str().unwrap(), "--subdir", "linux"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::write(temp.path().join("one"), "1").unwrap();
    fs::write(temp.path().join("two"), "2").unwrap();
    let list = temp.path().join("files.txt");
    fs::write(&list, "# relative to this file\n\none first\ntwo second\n").unwrap();
    let add = run(&["add", "--from", list.to_str().unwrap()]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert_eq!(fs::read_to_string(repo.join("linux/first")).unwrap(), "1");
    assert_eq!(fs::read_to_string(repo.join("linux/second")).unwrap(), "2");
    let repository = Repository::open(&repo).unwrap();
    repository
        .config()
        .unwrap()
        .set_str("user.name", "Test")
        .unwrap();
    repository
        .config()
        .unwrap()
        .set_str("user.email", "test@example.invalid")
        .unwrap();
    let commit = run(&["commit"]);
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let message = repository
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .message()
        .unwrap()
        .to_owned();
    assert!(message.contains("add \"linux/first\""), "{message}");
    assert!(message.contains("add \"linux/second\""), "{message}");
    assert!(!run(&["init", repo.to_str().unwrap()]).status.success());
    assert!(run(&["remove", "1"]).status.success());
    assert!(repo.join("linux/first").exists());
}

#[test]
fn list_import_supports_spaces_quoted_paths_and_literal_variables() {
    let f = Fixture::new("macos", false);
    let mut config = f.store.config().unwrap();
    config.entries.clear();
    f.store.save_config(&config).unwrap();
    f.write("notes one", "notes\n");
    f.write("scripts two/build.sh", "build\n");
    f.write("$literal", "literal\n");
    let list = f._temp.path().join("files.txt");
    fs::write(&list, "# quoted sources and targets\n\"source/notes one\" \"notes copy\"\n'source/scripts two' 'scripts copy' # directory\n'source/$literal' 'literal/$name'\n").unwrap();
    let output = f.cli(&["add", "--from", list.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (path, expected) in [
        ("notes copy", "notes\n"),
        ("scripts copy/build.sh", "build\n"),
        ("literal/$name", "literal\n"),
    ] {
        assert_eq!(
            fs::read_to_string(f.repository.join("macos").join(path)).unwrap(),
            expected
        );
    }
}

#[test]
fn malformed_list_reports_line_number_without_partial_import() {
    let f = Fixture::new("", false);
    let mut config = f.store.config().unwrap();
    config.entries.clear();
    f.store.save_config(&config).unwrap();
    f.write("a", "first entry");
    let before = fs::read(f.store.root.join("config.toml")).unwrap();
    let list = f._temp.path().join("files.txt");
    for invalid in ["unquoted path target", "\"unclosed"] {
        fs::write(&list, format!("source/a first\n{invalid}\n")).unwrap();
        let output = f.cli(&["add", "--from", list.to_str().unwrap()]);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("files.txt:2"), "{error}");
        assert_eq!(fs::read(f.store.root.join("config.toml")).unwrap(), before);
        assert!(!f.repository.join("first").exists());
        assert!(!f.store.root.join("state.db").exists());
    }
}

#[test]
fn dry_run_preserves_existing_sqlite_state() {
    let f = Fixture::new("", false);
    f.write("a", "before");
    f.sync();
    let before = fs::read(f.store.root.join("state.db")).unwrap();
    f.write("a", "after");
    let report = filetrail::sync::run(&f.store, true, None).unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|action| action == "update config/a")
    );
    assert_eq!(fs::read(f.store.root.join("state.db")).unwrap(), before);
    assert_eq!(fs::read_to_string(f.target("a")).unwrap(), "before");
}

#[test]
fn single_file_ownership_modification_and_deletion() {
    let f = Fixture::new("macos", true);
    f.write("shellrc", "original\n");
    let mut config = f.store.config().unwrap();
    config.entries[0].source.push("shellrc");
    config.entries[0].directory = false;
    config.entries[0].target = ".zshrc".into();
    f.store.save_config(&config).unwrap();
    f.sync();
    assert!(f.store.state().unwrap().owned.contains_key("macos/.zshrc"));
    filetrail::git::commit(&f.store, None, &[]).unwrap();
    f.write("shellrc", "changed\n");
    f.sync();
    assert!(
        filetrail::git::diff(&f.store, &["macos".into()])
            .unwrap()
            .contains("+changed")
    );
    fs::remove_file(f.source.join("shellrc")).unwrap();
    f.sync();
    assert!(!f.repository.join("macos/.zshrc").exists());
    let message = filetrail::git::commit(&f.store, None, &[]).unwrap();
    assert!(message.contains("delete \"macos/.zshrc\""), "{message}");
}

#[test]
fn external_sources_default_to_absolute_hierarchy_for_cli_and_lists() {
    for subdir in ["", "macos"] {
        for import_list in [false, true] {
            let f = Fixture::new(subdir, false);
            let mut config = f.store.config().unwrap();
            config.entries.clear();
            f.store.save_config(&config).unwrap();
            f.write("one", "single file\n");
            f.write("tools/build.sh", "directory child\n");
            if import_list {
                let list = f._temp.path().join("sources.txt");
                fs::write(&list, "source/one\nsource/tools\n").unwrap();
                let result = f.cli(&["add", "--from", list.to_str().unwrap()]);
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            } else {
                for source in [f.source.join("one"), f.source.join("tools")] {
                    let result = f.cli(&["add", source.to_str().unwrap()]);
                    assert!(
                        result.status.success(),
                        "{}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
            }
            let source = fs::canonicalize(&f.source).unwrap();
            let target = f
                .repository
                .join(subdir)
                .join(source.strip_prefix("/").unwrap());
            assert_eq!(
                fs::read_to_string(target.join("one")).unwrap(),
                "single file\n"
            );
            assert_eq!(
                fs::read_to_string(target.join("tools/build.sh")).unwrap(),
                "directory child\n"
            );
            let config = f.store.config().unwrap();
            assert_eq!(config.entries.len(), 2);
            for entry in &config.entries {
                assert_eq!(entry.target, entry.source.strip_prefix("/").unwrap());
            }
            let commit = filetrail::git::commit(&f.store, None, &[]).unwrap();
            assert!(commit.contains("sync 2 files"), "{commit}");
        }
    }
}

#[test]
fn empty_directories_are_copied_but_not_committed() {
    let f = Fixture::new("", false);
    fs::create_dir(f.source.join("empty")).unwrap();
    f.sync();
    assert!(f.target("empty").is_dir());
    assert!(filetrail::git::commit(&f.store, None, &[]).is_err());
}

#[test]
fn target_deletion_and_conflicted_deletion_are_protected() {
    let f = Fixture::new("", true);
    f.write("a", "initial");
    f.write("b", "initial");
    f.sync();
    fs::remove_file(f.target("a")).unwrap();
    fs::write(f.target("b"), "external edit").unwrap();
    fs::remove_file(f.source.join("b")).unwrap();
    assert!(
        !filetrail::sync::run(&f.store, false, None)
            .unwrap()
            .errors
            .is_empty()
    );
    assert!(!f.target("a").exists());
    assert_eq!(fs::read_to_string(f.target("b")).unwrap(), "external edit");
    filetrail::sync::run(&f.store, false, Some(Path::new("config/a"))).unwrap();
    assert!(f.store.state().unwrap().conflicts.contains_key("config/b"));
    assert_eq!(fs::read_to_string(f.target("b")).unwrap(), "external edit");
}

#[test]
fn source_exclusions_do_not_turn_a_single_file_into_a_deletion() {
    let f = Fixture::new("", true);
    f.write("a.tmp", "keep");
    let mut config = f.store.config().unwrap();
    config.entries[0].source.push("a.tmp");
    config.entries[0].directory = false;
    config.entries[0].target = "renamed".into();
    f.store.save_config(&config).unwrap();
    f.sync();
    config.entries[0].exclude = vec!["*.tmp".into()];
    f.store.save_config(&config).unwrap();
    f.sync();
    assert!(f.repository.join("renamed").exists());
}

#[test]
fn target_gitignore_does_not_force_add_ignored_files() {
    let f = Fixture::new("", false);
    fs::write(f.repository.join(".gitignore"), "config/ignored\n").unwrap();
    f.write("ignored", "value");
    f.sync();
    assert!(
        filetrail::git::status(&f.store)
            .unwrap()
            .contains("[ignored] config/ignored")
    );
    assert!(filetrail::git::commit(&f.store, None, &[]).is_err());
}

#[test]
fn case_insensitive_target_collisions_are_rejected() {
    let f = Fixture::new("", false);
    let mut config = f.store.config().unwrap();
    let mut other = config.entries[0].clone();
    other.id = 2;
    other.source = f.source.with_extension("other");
    other.target = "CONFIG/child".into();
    config.entries.push(other);
    assert!(f.store.save_config(&config).is_err());
}

struct DaemonGuard(Store);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = filetrail::daemon::stop(&self.0);
    }
}

#[test]
fn background_start_polling_and_singleton() {
    let f = Fixture::new("", false);
    f.write("a", "initial");
    let started = f.cli(&["daemon", "start", "--poll"]);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let _guard = DaemonGuard(f.store.clone());
    let status = String::from_utf8(started.stdout).unwrap();
    assert!(status.contains("mode=poll"));
    assert_eq!(
        String::from_utf8(f.cli(&["daemon", "start"]).stdout).unwrap(),
        status
    );
    assert!(!f.cli(&["daemon", "run"]).status.success());
    wait_until(|| f.target("a").exists());
    for name in [
        "config.toml",
        "state.db",
        "operation.lock",
        "daemon.lock",
        "daemon.sock",
        "daemon-output.log",
        "filetrail.log",
    ] {
        wait_until(|| f.store.root.join(name).exists());
    }
    f.write("a", "polled change");
    wait_until(|| {
        fs::read_to_string(f.target("a")).is_ok_and(|content| content == "polled change")
    });
    assert!(f.cli(&["daemon", "stop"]).status.success());
    assert!(!f.cli(&["daemon", "status"]).status.success());
}
