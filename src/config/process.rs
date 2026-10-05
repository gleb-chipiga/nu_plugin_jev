//! Resolves immutable process policy separately from caller-scoped configuration.

use std::{
    ffi::OsStr,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use nu_protocol::LabeledError;
use tokio::sync::Semaphore;

use crate::error::JevError;

use super::{Source, config_error, parse_unsigned, read_nuon, resolve_path, same_file};

/// Names the startup-only aggregate HTTP attempt setting.
const LIMIT_ENV: &str = "NU_PLUGIN_JEV_MAX_IN_FLIGHT";

/// Holds an immutable process capacity representable by the shared attempt semaphore.
/// This is not an invocation setting: callers cannot resize it through request overrides.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HttpAttemptLimit(NonZeroUsize);

impl HttpAttemptLimit {
    /// Resolves startup environment and NUON layers on the synchronous startup thread.
    /// Only absence permits fallback; a selected invalid value fails before command servicing.
    pub(crate) fn from_startup() -> Result<Self, LabeledError> {
        // No invocation EngineInterface exists before serve_plugin. Process environment is
        // correct only for this immutable startup policy, not later caller-scoped settings.
        // Resolve the highest-priority value first, including its errors. Reading fallback
        // files eagerly would make an unrelated malformed file defeat a valid override.
        if let Some(value) = std::env::var_os(LIMIT_ENV) {
            return Self::from_value(Some(&value)).map_err(JevError::into_labeled);
        }
        let startup_dir = startup_directory(std::env::var_os("PWD").as_deref())?;
        let selected = std::env::var_os("NU_PLUGIN_JEV_CONFIG");
        let user_root = startup_user_config_dir()?;
        Self::from_files(&startup_dir, selected.as_deref().map(Path::new), &user_root)
    }

    /// Selects startup files without changing cwd or retaining any file credentials.
    fn from_files(
        startup_dir: &Path,
        selected: Option<&Path>,
        user_root: &Path,
    ) -> Result<Self, LabeledError> {
        if selected.is_some_and(|path| path.as_os_str().is_empty()) {
            return Err(config_error("NU_PLUGIN_JEV_CONFIG must be a nonempty path"));
        }
        let local_path = selected
            .map(|path| resolve_path(startup_dir, path))
            .unwrap_or_else(|| startup_dir.join(".nu_plugin_jev.nuon"));
        let user_path = user_root.join("nu_plugin_jev/config.nuon");
        if same_file(&local_path, &user_path) {
            // A selected user file remains a user layer, not an implicit local file;
            // applying local restrictions would reject its legitimate credential settings.
            return Self::from_file(&user_path, selected.is_some(), false, Source::User)
                .map(|limit| limit.unwrap_or_default());
        }
        // Missing max_in_flight allows user fallback. Invalid selected values propagate
        // through ? instead: quietly falling back would make the process policy ambiguous.
        if let Some(limit) = Self::from_file(
            &local_path,
            selected.is_some(),
            selected.is_none(),
            Source::Local,
        )? {
            return Ok(limit);
        }
        Self::from_file(&user_path, false, false, Source::User)
            .map(|limit| limit.unwrap_or_default())
    }

    /// Validates only a selected native file integer using the existing safe NUON reader.
    fn from_file(
        path: &Path,
        required: bool,
        implicit_local: bool,
        source: Source,
    ) -> Result<Option<Self>, LabeledError> {
        let label = if source == Source::Local {
            "local"
        } else {
            "user"
        };
        let Some(file) = read_nuon(path, required, implicit_local, label)? else {
            return Ok(None);
        };
        // Retain only the validated capacity, not FileSettings or its parsed API key.
        // Request credentials continue to resolve independently for each invocation.
        file.values
            .get("max_in_flight")
            .map(|value| {
                let capacity = parse_unsigned(value, "max_in_flight", source, false)?;
                Self::new(capacity).map_err(|_| {
                    config_error(format!(
                        "{source} max_in_flight exceeds the supported HTTP attempt capacity"
                    ))
                })
            })
            .transpose()
    }

    /// Rejects zero and values above Semaphore::MAX_PERMITS before semaphore construction.
    pub(crate) fn new(capacity: usize) -> Result<Self, JevError> {
        NonZeroUsize::new(capacity)
            .filter(|value| value.get() <= Semaphore::MAX_PERMITS)
            .map(Self)
            .ok_or_else(Self::invalid)
    }

    /// Returns the validated number of concurrent HTTP attempts.
    pub(crate) fn get(self) -> usize {
        self.0.get()
    }

    /// Parses an optional startup value without retaining it in diagnostics.
    fn from_value(value: Option<&OsStr>) -> Result<Self, JevError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let value = value.to_str().ok_or_else(Self::invalid)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Self::invalid());
        }
        Self::new(value.parse().map_err(|_| Self::invalid())?)
    }

    /// Describes valid input without echoing environment contents.
    fn invalid() -> JevError {
        JevError::Validation(concat!(
            "NU_PLUGIN_JEV_MAX_IN_FLIGHT must be a positive base-10 integer ",
            "within the supported HTTP attempt capacity"
        ))
    }
}

impl Default for HttpAttemptLimit {
    /// Keeps the process default independent of invocation-local jobs.
    fn default() -> Self {
        // Deriving this from jobs would couple one invocation's work window to the
        // combined network activity of every concurrently executing command.
        Self(NonZeroUsize::new(128).expect("nonzero default HTTP attempt limit"))
    }
}

/// Uses Nu's startup PWD because Nu deliberately spawns plugins beside their executable.
fn startup_directory(pwd: Option<&OsStr>) -> Result<PathBuf, LabeledError> {
    // Relative PWD cannot identify Nu's original directory independently of executable cwd.
    // Accept only an absolute startup PWD; do not resolve it relative to the binary directory.
    if let Some(path) = pwd.map(PathBuf::from).filter(|path| path.is_absolute()) {
        return Ok(path);
    }
    std::env::current_dir().map_err(|_| config_error("cannot determine plugin startup directory"))
}

/// Finds the user NUON root from process-startup platform settings, not caller overrides.
fn startup_user_config_dir() -> Result<PathBuf, LabeledError> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path);
    }
    #[cfg(unix)]
    let platform_root = dirs::home_dir().map(|home| home.join(".config"));
    #[cfg(not(unix))]
    let platform_root = dirs::config_dir();
    platform_root.ok_or_else(|| config_error("cannot find startup user config directory"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use nu_protocol::{Record, Value};

    use crate::config::{ConfigScope, ConfigSources, read_nuon, resolve, resolve_models};

    use super::{HttpAttemptLimit, LIMIT_ENV, OsStr, Semaphore, startup_directory};

    /// Owns only synthetic startup NUON files under the existing target directory.
    struct Fixture(PathBuf);

    impl Fixture {
        /// Creates distinct local and user roots without modifying process environment.
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "jev-startup-config-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir(&dir).unwrap();
            fs::create_dir_all(dir.join("user/nu_plugin_jev")).unwrap();
            Self(dir)
        }

        /// Writes a synthetic file relative to this fixture's unique directory.
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).unwrap();
            path
        }

        /// Resolves only this fixture's startup file layers.
        fn limit(
            &self,
            selected: Option<&Path>,
        ) -> Result<HttpAttemptLimit, nu_protocol::LabeledError> {
            HttpAttemptLimit::from_files(&self.0, selected, &self.0.join("user"))
        }
    }

    impl Drop for Fixture {
        /// Removes the fixture-owned files, never a Cargo build directory.
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// Accepts positive capacities and uses 128 only when the setting is absent.
    #[test]
    fn startup_limit_defaults_and_validates_capacity() {
        assert_eq!(HttpAttemptLimit::from_value(None).unwrap().get(), 128);
        for capacity in [1, 2, 128, Semaphore::MAX_PERMITS] {
            let value = capacity.to_string();
            assert_eq!(
                HttpAttemptLimit::from_value(Some(OsStr::new(&value)))
                    .unwrap()
                    .get(),
                capacity
            );
        }
    }

    /// Rejects unsupported input with a redacted setting-specific error.
    #[test]
    fn invalid_startup_limits_never_echo_values() {
        let oversized = (Semaphore::MAX_PERMITS + 1).to_string();
        for value in [
            "",
            "0",
            "-1",
            "2.5",
            "private-value",
            "+2",
            " 2",
            "2 ",
            "٢",
            &oversized,
            "999999999999999999999999999999999999999999999999",
        ] {
            let error = HttpAttemptLimit::from_value(Some(OsStr::new(value))).unwrap_err();
            let diagnostic = error.to_string();
            assert_eq!(error.kind_name(), "validation");
            assert!(diagnostic.contains(LIMIT_ENV));
            if value == "private-value" {
                assert!(!diagnostic.contains(value));
            }
        }
        assert!(HttpAttemptLimit::new(0).is_err());
        assert!(HttpAttemptLimit::new(usize::MAX).is_err());
    }

    /// Non-Unicode startup values fail without lossy conversion or a panic.
    #[cfg(unix)]
    #[test]
    fn non_unicode_startup_limit_is_invalid() {
        use std::os::unix::ffi::OsStrExt;

        let value = OsStr::from_bytes(b"private-\xff-value");
        let error = HttpAttemptLimit::from_value(Some(value)).unwrap_err();
        assert!(error.to_string().contains(LIMIT_ENV));
        assert!(!error.to_string().contains("private"));
    }

    /// Resolves partial local/user layers and skips lower files after selecting a limit.
    #[test]
    fn startup_file_limits_follow_per_field_precedence() {
        let fixture = Fixture::new();
        assert_eq!(fixture.limit(None).unwrap().get(), 128);
        fixture.write(".nu_plugin_jev.nuon", "{jobs: 8}");
        fixture.write("user/nu_plugin_jev/config.nuon", "{max_in_flight: 4}");
        assert_eq!(fixture.limit(None).unwrap().get(), 4);
        fixture.write(".nu_plugin_jev.nuon", "{max_in_flight: 2}");
        assert_eq!(fixture.limit(None).unwrap().get(), 2);
        fixture.write("user/nu_plugin_jev/config.nuon", "SECRET_DO_NOT_SHOW {");
        assert_eq!(fixture.limit(None).unwrap().get(), 2);
    }

    /// Uses an absolute supplied PWD and falls back to actual cwd for absent or relative PWD.
    #[test]
    fn startup_directory_uses_nu_pwd_without_changing_cwd() {
        let fixture = Fixture::new();
        assert_eq!(
            startup_directory(Some(fixture.0.as_os_str())).unwrap(),
            fixture.0
        );
        for pwd in [None, Some(OsStr::new("relative")), Some(OsStr::new(""))] {
            assert_eq!(
                startup_directory(pwd).unwrap(),
                std::env::current_dir().unwrap()
            );
        }
    }

    /// Anchors explicit files to startup cwd and retains the existing implicit-file policy.
    #[test]
    fn startup_files_use_explicit_selection_and_safe_paths() {
        let fixture = Fixture::new();
        fixture.write(".nu_plugin_jev.nuon", "{max_in_flight: 9}");
        let custom = fixture.write(
            "custom.nuon",
            "{max_in_flight: 2, base_url: 'https://example.test'}",
        );
        assert_eq!(
            fixture.limit(Some(Path::new("custom.nuon"))).unwrap().get(),
            2
        );
        assert_eq!(fixture.limit(Some(&custom)).unwrap().get(), 2);
        assert!(fixture.limit(Some(Path::new("missing.nuon"))).is_err());
        assert!(fixture.limit(Some(Path::new(""))).is_err());
        fixture.write(".nu_plugin_jev.nuon", "{max_in_flight: 2, proxy: direct}");
        assert!(fixture.limit(None).is_err());
    }

    /// Rejects selected file types and capacities without falling through to a user value.
    #[test]
    fn invalid_file_limits_are_redacted_and_do_not_fall_back() {
        let fixture = Fixture::new();
        fixture.write("user/nu_plugin_jev/config.nuon", "{max_in_flight: 4}");
        let oversized = (Semaphore::MAX_PERMITS + 1).to_string();
        for value in [
            "0",
            "-1",
            "2.5",
            "true",
            "null",
            "'PRIVATE_VALUE'",
            "'2'",
            &oversized,
        ] {
            fixture.write(
                ".nu_plugin_jev.nuon",
                &format!("{{max_in_flight: {value}}}"),
            );
            let diagnostic = fixture.limit(None).unwrap_err().to_string();
            assert!(diagnostic.contains("local NUON max_in_flight"));
            assert!(!diagnostic.contains("PRIVATE_VALUE"));
        }
        fs::remove_file(fixture.0.join(".nu_plugin_jev.nuon")).unwrap();
        fixture.write(
            "user/nu_plugin_jev/config.nuon",
            "{max_in_flight: 'PRIVATE_VALUE'}",
        );
        let diagnostic = fixture.limit(None).unwrap_err().to_string();
        assert!(diagnostic.contains("user NUON max_in_flight"));
        assert!(!diagnostic.contains("PRIVATE_VALUE"));
    }

    /// Reuses bounded, data-only parsing while suppressing sensitive parser input.
    #[test]
    fn startup_files_keep_nuon_parser_safeguards() {
        let fixture = Fixture::new();
        let oversized = format!("{{}}{}", " ".repeat(65_537));
        for contents in [
            "{api_key: 'PRIVATE_VALUE' trailing}",
            "{PRIVATE_VALUE: 2}",
            "{max_in_flight: ($env.PRIVATE_VALUE)}",
            "{max_in_flight: 2, max_in_flight: 4}",
            &oversized,
        ] {
            fixture.write(".nu_plugin_jev.nuon", contents);
            let diagnostic = format!("{:?}", fixture.limit(None).unwrap_err());
            assert!(!diagnostic.contains("PRIVATE_VALUE"));
        }
    }

    /// Recognizing a process-only file field does not validate or apply it during commands.
    #[test]
    fn invocation_settings_ignore_process_only_limit() {
        let fixture = Fixture::new();
        let path = fixture.write(".nu_plugin_jev.nuon", "{max_in_flight: 'ignored', jobs: 8}");
        let sources = ConfigSources {
            local: read_nuon(&path, true, true, "local").unwrap(),
            plugin: Some(Value::test_record(Record::from_iter([(
                "max_in_flight".into(),
                Value::test_int(1),
            )]))),
            ..ConfigSources::default()
        };
        assert_eq!(
            resolve(&sources, ConfigScope::Table)
                .unwrap()
                .jobs
                .unwrap()
                .get(),
            8
        );
        assert!(resolve(&sources, ConfigScope::Single).is_ok());
        assert!(resolve_models(&sources).is_ok());
    }

    /// A shared local/user path is read as the user layer, matching invocation behavior.
    #[cfg(unix)]
    #[test]
    fn startup_same_file_retains_user_layer_rules() {
        let fixture = Fixture::new();
        let user = fixture.write(
            "user/nu_plugin_jev/config.nuon",
            "{max_in_flight: 2, proxy: direct}",
        );
        std::os::unix::fs::symlink(&user, fixture.0.join(".nu_plugin_jev.nuon")).unwrap();
        assert_eq!(fixture.limit(None).unwrap().get(), 2);
    }
}
