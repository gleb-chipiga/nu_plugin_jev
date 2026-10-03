//! Resolves caller-scoped settings once per invocation without retaining credentials globally.

use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::Read,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

use nu_plugin::{EngineInterface, EvaluatedCall};
use nu_protocol::{LabeledError, Record, Span, Value};
use reqwest::Url;

/// Identifies which settings apply to the current command family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigScope {
    /// A single-state evaluation also needs a model.
    Single,
    /// A table evaluation additionally needs scheduling and cache limits.
    Table,
    /// Model discovery needs transport settings but no evaluation model.
    Models,
}

/// Holds the settings shared by evaluation and model discovery HTTP calls.
#[derive(Clone, Debug)]
pub(crate) struct TransportConfig {
    /// Authenticated service root, without a query or fragment.
    pub(crate) base_url: Url,
    /// Effective proxy policy selected for this invocation.
    pub(crate) proxy: ProxyPolicy,
    /// Total deadline for one logical request, including retry waits.
    pub(crate) timeout: Duration,
    /// Number of additional HTTP attempts after the first.
    pub(crate) retries: usize,
}

/// Exposes the common transport settings without changing evaluation config shape.
pub(crate) trait TransportSettings {
    /// Returns the validated transport settings for one invocation.
    fn transport(&self) -> (&Url, Duration, usize);
}

impl TransportSettings for TransportConfig {
    /// Borrows model-discovery transport settings.
    fn transport(&self) -> (&Url, Duration, usize) {
        (&self.base_url, self.timeout, self.retries)
    }
}

/// Holds the bounded completed-cache limits for one table invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheLimits {
    /// Maximum number of completed successful entries.
    pub(crate) max_entries: NonZeroUsize,
    /// Maximum approximate accounted bytes of completed entries.
    pub(crate) max_approx_bytes: NonZeroUsize,
}

/// Selects standard discovery, no proxy, or one authoritative proxy endpoint.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum ProxyPolicy {
    /// Use the process-startup client with ordinary environment and OS discovery.
    Auto,
    /// Bypass all proxy discovery and connect to the service directly.
    Direct,
    /// Route every request through the specified HTTP or SOCKS5h proxy.
    Explicit(String),
}

impl fmt::Debug for ProxyPolicy {
    /// Hides proxy endpoints and credentials from derived configuration diagnostics.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("Auto"),
            Self::Direct => formatter.write_str("Direct"),
            Self::Explicit(_) => formatter.write_str("Explicit(<redacted>)"),
        }
    }
}

/// Holds validated immutable settings for one plugin command invocation.
#[derive(Clone, Debug)]
pub(crate) struct InvocationConfig {
    /// Requested model for every evaluation command.
    pub(crate) model: String,
    /// Authenticated service root, without a query or fragment.
    pub(crate) base_url: Url,
    /// Effective proxy policy selected for this invocation.
    pub(crate) proxy: ProxyPolicy,
    /// Total deadline for each logical evaluation, including retry waits.
    pub(crate) timeout: Duration,
    /// Maximum unique evaluations in a table invocation.
    pub(crate) jobs: Option<NonZeroUsize>,
    /// Number of additional HTTP attempts after the first.
    pub(crate) retries: usize,
    /// Invocation-local completed-cache limits for table commands.
    pub(crate) cache: Option<CacheLimits>,
}

impl TransportSettings for InvocationConfig {
    /// Borrows evaluation transport settings.
    fn transport(&self) -> (&Url, Duration, usize) {
        (&self.base_url, self.timeout, self.retries)
    }
}

/// Holds a caller-scoped bearer credential without exposing it through Debug.
pub(crate) struct ApiKey(String);

impl ApiKey {
    /// Borrows the credential only for an authenticated HTTP request.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a credential for local mock-server tests only.
    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Captures only the flag, plugin-config, and environment values applicable to a call.
#[derive(Default)]
pub(crate) struct ConfigSources {
    /// Evaluated command flags by their long names.
    pub(crate) flags: BTreeMap<&'static str, Value>,
    /// Plugin-specific caller configuration, if supplied.
    pub(crate) plugin: Option<Value>,
    /// Caller environment values by their variable names.
    pub(crate) env: BTreeMap<&'static str, Value>,
    /// Explicitly supplied caller credential, including an invalid value.
    key: Option<Value>,
    /// Selected local file, read once before any rows are consumed.
    local: Option<TomlSettings>,
    /// Optional platform user file, read once before any rows are consumed.
    user: Option<TomlSettings>,
}

/// Keeps parsed file values and key-file permission state private to one invocation.
#[derive(Default)]
struct TomlSettings {
    values: BTreeMap<String, Value>,
    key: Option<Value>,
    insecure_key_permissions: bool,
}

/// Maximum accepted size of each configuration file in bytes.
const MAX_TOML_BYTES: u64 = 65_536;

/// Captures configuration through the public engine interface before row consumption.
pub(crate) fn capture_sources(
    engine: &EngineInterface,
    call: &EvaluatedCall,
    scope: ConfigScope,
) -> Result<ConfigSources, LabeledError> {
    let flag_names: &'static [&'static str] = match scope {
        ConfigScope::Single => &["model", "base-url", "timeout", "config"],
        ConfigScope::Table => &["model", "base-url", "timeout", "jobs", "config"],
        ConfigScope::Models => &["base-url", "timeout", "config"],
    };
    let env_names: &'static [&'static str] = match scope {
        ConfigScope::Single => &[
            "NU_PLUGIN_JEV_MODEL",
            "NU_PLUGIN_JEV_BASE_URL",
            "NU_PLUGIN_JEV_TIMEOUT_MS",
            "NU_PLUGIN_JEV_RETRIES",
            "NU_PLUGIN_JEV_PROXY",
        ],
        ConfigScope::Table => &[
            "NU_PLUGIN_JEV_MODEL",
            "NU_PLUGIN_JEV_BASE_URL",
            "NU_PLUGIN_JEV_TIMEOUT_MS",
            "NU_PLUGIN_JEV_JOBS",
            "NU_PLUGIN_JEV_RETRIES",
            "NU_PLUGIN_JEV_PROXY",
        ],
        ConfigScope::Models => &[
            "NU_PLUGIN_JEV_BASE_URL",
            "NU_PLUGIN_JEV_TIMEOUT_MS",
            "NU_PLUGIN_JEV_RETRIES",
            "NU_PLUGIN_JEV_PROXY",
        ],
    };
    let flags: BTreeMap<&'static str, Value> = flag_names
        .iter()
        .filter_map(|name| call.get_flag_value(name).map(|value| (*name, value)))
        .collect();
    let plugin = engine.get_plugin_config().map_err(LabeledError::from)?;
    let env = env_names
        .iter()
        .map(|name| {
            engine
                .get_env_var(*name)
                .map_err(LabeledError::from)
                .map(|value| value.map(|value| (*name, value)))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let key = engine
        .get_env_var("TYPESAFE_API_KEY")
        .map_err(LabeledError::from)?;
    let caller_dir = match engine.get_current_dir() {
        Ok(dir) => PathBuf::from(dir),
        #[cfg(test)]
        Err(_) => std::env::current_dir()
            .map_err(|_| config_error("cannot determine test caller directory"))?,
        #[cfg(not(test))]
        Err(error) => return Err(LabeledError::from(error)),
    };
    let selected_path = if let Some(value) = flags.get("config") {
        Some(config_path_value(value, "--config")?)
    } else {
        engine
            .get_env_var("NU_PLUGIN_JEV_CONFIG")
            .map_err(LabeledError::from)?
            .map(|value| config_path_value(&value, "NU_PLUGIN_JEV_CONFIG"))
            .transpose()?
    };
    let explicit = selected_path.is_some();
    let local_path = selected_path
        .map(|path| resolve_path(&caller_dir, &path))
        .unwrap_or_else(|| caller_dir.join(".nu_plugin_jev.toml"));
    let xdg = engine
        .get_env_var("XDG_CONFIG_HOME")
        .map_err(LabeledError::from)?;
    let user_root = match xdg {
        Some(Value::String { val, .. }) if Path::new(&val).is_absolute() => PathBuf::from(val),
        _ => default_user_config_dir(engine)?,
    };
    let user_path = user_root.join("nu_plugin_jev/config.toml");
    let same_file = local_path == user_path
        || local_path.canonicalize().ok().is_some_and(|local| {
            user_path
                .canonicalize()
                .ok()
                .is_some_and(|user| local == user)
        });
    let user = read_toml(&user_path, same_file && explicit, false, "user")?;
    let local = if same_file {
        None
    } else {
        read_toml(&local_path, explicit, !explicit, "local")?
    };
    Ok(ConfigSources {
        flags,
        plugin,
        env,
        key,
        local,
        user,
    })
}

/// Finds the platform user config root without trusting stale process XDG settings.
fn default_user_config_dir(engine: &EngineInterface) -> Result<PathBuf, LabeledError> {
    #[cfg(unix)]
    {
        if let Some(Value::String { val, .. }) =
            engine.get_env_var("HOME").map_err(LabeledError::from)?
            && Path::new(&val).is_absolute()
        {
            return Ok(PathBuf::from(val).join(".config"));
        }
        dirs::home_dir()
            .map(|home| home.join(".config"))
            .ok_or_else(|| config_error("cannot find user config directory"))
    }
    #[cfg(not(unix))]
    dirs::config_dir().ok_or_else(|| config_error("cannot find user config directory"))
}

/// Validates a selected file path without displaying its value.
fn config_path_value(value: &Value, source: &str) -> Result<PathBuf, LabeledError> {
    match value {
        Value::String { val, .. } if !val.is_empty() => Ok(PathBuf::from(val)),
        _ => Err(config_error(format!("{source} must be a nonempty path"))),
    }
}

/// Anchors relative paths to the caller's directory without changing process cwd.
fn resolve_path(caller_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        caller_dir.join(path)
    }
}

/// Reads and checks one bounded TOML file without exposing its contents in errors.
fn read_toml(
    path: &Path,
    required: bool,
    implicit_local: bool,
    label: &str,
) -> Result<Option<TomlSettings>, LabeledError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(config_error(format!(
                "cannot read {label} TOML configuration"
            )));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|_| config_error(format!("cannot inspect {label} TOML configuration")))?;
    if !metadata.is_file() || metadata.len() > MAX_TOML_BYTES {
        return Err(config_error(format!(
            "{label} TOML configuration must be a regular file of at most {MAX_TOML_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity((metadata.len() + 1) as usize);
    file.take(MAX_TOML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| config_error(format!("cannot read {label} TOML configuration")))?;
    if bytes.len() as u64 > MAX_TOML_BYTES {
        return Err(config_error(format!(
            "{label} TOML configuration exceeds {MAX_TOML_BYTES} bytes"
        )));
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|_| config_error(format!("{label} TOML configuration is not UTF-8")))?;
    let table: toml::Table = toml::from_str(source).map_err(|error: toml::de::Error| {
        let location = error
            .span()
            .map(|span| {
                format!(
                    " at line {}",
                    source[..span.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count()
                        + 1
                )
            })
            .unwrap_or_default();
        config_error(format!("malformed {label} TOML configuration{location}"))
    })?;
    let mut settings = TomlSettings::default();
    for (name, value) in table {
        match name.as_str() {
            "api_key" => settings.key = Some(toml_value(value)),
            "cache" => {
                let toml::Value::Table(cache) = value else {
                    settings.values.insert(name, toml_value(value));
                    continue;
                };
                let mut record = Record::new();
                for (cache_name, cache_value) in cache {
                    if !matches!(cache_name.as_str(), "max_entries" | "max_approx_bytes") {
                        return Err(config_error(format!("unknown {label} TOML cache field")));
                    }
                    record.push(cache_name, toml_value(cache_value));
                }
                settings
                    .values
                    .insert(name, Value::record(record, Span::unknown()));
            }
            "base_url" | "proxy" if implicit_local => {
                return Err(config_error(format!(
                    "implicit local TOML cannot set {name}"
                )));
            }
            "model" | "base_url" | "timeout_ms" | "jobs" | "retries" | "proxy" => {
                settings.values.insert(name, toml_value(value));
            }
            _ => return Err(config_error(format!("unknown {label} TOML field"))),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        settings.insecure_key_permissions =
            settings.key.is_some() && metadata.permissions().mode() & 0o077 != 0;
    }
    Ok(Some(settings))
}

/// Converts TOML settings into Nu scalars for the existing selected-value validators.
fn toml_value(value: toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::string(value, Span::unknown()),
        toml::Value::Integer(value) => Value::int(value, Span::unknown()),
        _ => Value::nothing(Span::unknown()),
    }
}

/// Resolves each applicable setting independently using the documented priority.
pub(crate) fn resolve(
    sources: &ConfigSources,
    scope: ConfigScope,
) -> Result<InvocationConfig, LabeledError> {
    let plugin = plugin_record(sources)?;
    let model = match selected(
        sources,
        plugin,
        Some("model"),
        "model",
        Some("NU_PLUGIN_JEV_MODEL"),
    ) {
        Some((source, value)) => parse_nonempty_string(value, "model", source)?,
        None => "jev-latest".to_owned(),
    };
    let transport = resolve_transport(sources, plugin)?;
    let TransportConfig {
        base_url,
        proxy,
        timeout,
        retries,
    } = transport;
    let jobs = if scope == ConfigScope::Table {
        let value = selected(
            sources,
            plugin,
            Some("jobs"),
            "jobs",
            Some("NU_PLUGIN_JEV_JOBS"),
        );
        let jobs = match value {
            Some((source, value)) => parse_positive_usize(value, "jobs", source)?,
            None => NonZeroUsize::new(16).expect("nonzero default jobs"),
        };
        if jobs.get() > usize::MAX / 2 {
            return Err(config_error(
                "jobs is too large for a bounded admission window",
            ));
        }
        Some(jobs)
    } else {
        None
    };
    let cache = if scope == ConfigScope::Table {
        for file in [sources.local.as_ref(), sources.user.as_ref()]
            .into_iter()
            .flatten()
        {
            if file
                .values
                .get("cache")
                .is_some_and(|value| !matches!(value, Value::Record { .. }))
            {
                return Err(config_error("TOML cache must be a table"));
            }
        }
        let cache_record = match plugin.and_then(|record| record.get("cache")) {
            Some(Value::Record { val, .. }) => Some(&**val),
            None => None,
            Some(_) => return Err(config_error("plugin cache config must be a record")),
        };
        Some(CacheLimits {
            max_entries: cache_limit(sources, cache_record, "max_entries", 1024)?,
            max_approx_bytes: cache_limit(sources, cache_record, "max_approx_bytes", 16_777_216)?,
        })
    } else {
        None
    };
    Ok(InvocationConfig {
        model,
        base_url,
        proxy,
        timeout,
        jobs,
        retries,
        cache,
    })
}

/// Resolves only the transport settings required for a model-list request.
pub(crate) fn resolve_models(sources: &ConfigSources) -> Result<TransportConfig, LabeledError> {
    resolve_transport(sources, plugin_record(sources)?)
}

/// Borrows a caller plugin record while rejecting a wrong top-level shape.
fn plugin_record(sources: &ConfigSources) -> Result<Option<&Record>, LabeledError> {
    match sources.plugin.as_ref() {
        Some(Value::Record { val, .. }) => Ok(Some(val)),
        None => Ok(None),
        Some(_) => Err(config_error("plugin config must be a record")),
    }
}

/// Shares per-field transport precedence and validation across command families.
fn resolve_transport(
    sources: &ConfigSources,
    plugin: Option<&Record>,
) -> Result<TransportConfig, LabeledError> {
    let base_url = match selected(
        sources,
        plugin,
        Some("base-url"),
        "base_url",
        Some("NU_PLUGIN_JEV_BASE_URL"),
    ) {
        Some((source, value)) => parse_service_root(value, source)?,
        None => Url::parse("https://api.typesafe.ai").expect("valid default service root"),
    };
    let proxy = selected(sources, plugin, None, "proxy", Some("NU_PLUGIN_JEV_PROXY"))
        .map(|(source, value)| parse_proxy_policy(value, source))
        .transpose()?
        .unwrap_or(ProxyPolicy::Auto);
    let timeout = match selected(
        sources,
        plugin,
        Some("timeout"),
        "timeout",
        Some("NU_PLUGIN_JEV_TIMEOUT_MS"),
    ) {
        Some((source @ (Source::Environment | Source::Local | Source::User), value)) => {
            let millis = parse_unsigned(value, "timeout_ms", source, false)?;
            let millis =
                u64::try_from(millis).map_err(|_| config_error("timeout_ms is too large"))?;
            Duration::from_millis(millis)
        }
        Some((_, Value::Duration { val, .. })) if *val > 0 => Duration::from_nanos(*val as u64),
        Some((source, _)) => {
            return Err(config_error(format!(
                "{source} timeout must be a positive Nu duration"
            )));
        }
        None => Duration::from_secs(30),
    };
    if std::time::Instant::now().checked_add(timeout).is_none() {
        return Err(config_error("timeout is too large for a deadline"));
    }
    let retries = selected(
        sources,
        plugin,
        None,
        "retries",
        Some("NU_PLUGIN_JEV_RETRIES"),
    )
    .map(|(source, value)| parse_unsigned(value, "retries", source, true))
    .transpose()?
    .unwrap_or(3);
    Ok(TransportConfig {
        base_url,
        proxy,
        timeout,
        retries,
    })
}

/// Selects a live credential from the immutable caller and file snapshot.
pub(crate) fn require_api_key(sources: &ConfigSources) -> Result<ApiKey, LabeledError> {
    if sources
        .local
        .as_ref()
        .is_some_and(|file| file.insecure_key_permissions)
        || sources
            .user
            .as_ref()
            .is_some_and(|file| file.insecure_key_permissions)
    {
        return Err(config_error("key-bearing TOML file must be owner-only"));
    }
    parse_api_key(
        sources
            .key
            .clone()
            .or_else(|| sources.local.as_ref().and_then(|file| file.key.clone()))
            .or_else(|| sources.user.as_ref().and_then(|file| file.key.clone())),
    )
}

/// Validates a captured caller credential without copying it into an error.
fn parse_api_key(value: Option<Value>) -> Result<ApiKey, LabeledError> {
    match value {
        Some(Value::String { val, .. }) if !val.trim().is_empty() => Ok(ApiKey(val)),
        _ => Err(config_error(
            "a nonempty API key is required for a live Jev request",
        )),
    }
}

/// Labels the source of a selected setting for precise validation errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Flag,
    Plugin,
    Environment,
    Local,
    User,
}

impl std::fmt::Display for Source {
    /// Names the selected configuration layer without printing its value.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Flag => "flag",
            Self::Plugin => "plugin config",
            Self::Environment => "environment",
            Self::Local => "local TOML",
            Self::User => "user TOML",
        };
        formatter.write_str(name)
    }
}

/// Selects one setting without falling back after an invalid higher-priority value.
fn selected<'a>(
    sources: &'a ConfigSources,
    plugin: Option<&'a Record>,
    flag_key: Option<&'static str>,
    plugin_key: &'static str,
    env_key: Option<&'static str>,
) -> Option<(Source, &'a Value)> {
    flag_key
        .and_then(|key| sources.flags.get(key).map(|value| (Source::Flag, value)))
        .or_else(|| {
            plugin.and_then(|record| record.get(plugin_key).map(|value| (Source::Plugin, value)))
        })
        .or_else(|| {
            env_key.and_then(|key| {
                sources
                    .env
                    .get(key)
                    .map(|value| (Source::Environment, value))
            })
        })
        .or_else(|| {
            sources.local.as_ref().and_then(|file| {
                file.values
                    .get(if plugin_key == "timeout" {
                        "timeout_ms"
                    } else {
                        plugin_key
                    })
                    .map(|value| (Source::Local, value))
            })
        })
        .or_else(|| {
            sources.user.as_ref().and_then(|file| {
                file.values
                    .get(if plugin_key == "timeout" {
                        "timeout_ms"
                    } else {
                        plugin_key
                    })
                    .map(|value| (Source::User, value))
            })
        })
}

/// Validates a nonempty string selected for model or service root.
fn parse_nonempty_string(
    value: &Value,
    field: &str,
    source: Source,
) -> Result<String, LabeledError> {
    match value {
        Value::String { val, .. } if !val.trim().is_empty() => Ok(val.clone()),
        _ => Err(config_error(format!(
            "{source} {field} must be a nonempty string"
        ))),
    }
}

/// Validates an absolute HTTP(S) service root without credential-bearing components.
fn parse_service_root(value: &Value, source: Source) -> Result<Url, LabeledError> {
    let text = parse_nonempty_string(value, "base_url", source)?;
    let url = Url::parse(&text)
        .map_err(|_| config_error(format!("{source} base_url is not an absolute URL")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(config_error(format!(
            "{source} base_url must be an HTTP(S) root without credentials, query, or fragment"
        )));
    }
    Ok(url)
}

/// Validates a selected policy without reproducing a sensitive proxy URL.
fn parse_proxy_policy(value: &Value, source: Source) -> Result<ProxyPolicy, LabeledError> {
    let Value::String { val, .. } = value else {
        return Err(config_error(format!("{source} proxy must be a string")));
    };
    match val.as_str() {
        "auto" => Ok(ProxyPolicy::Auto),
        "direct" => Ok(ProxyPolicy::Direct),
        _ => {
            let url = Url::parse(val).map_err(|_| {
                config_error(format!(
                    "{source} proxy must be auto, direct, or a supported URL"
                ))
            })?;
            if !matches!(url.scheme(), "http" | "socks5h")
                || url.host().is_none()
                || url.query().is_some()
                || url.fragment().is_some()
                || !matches!(url.path(), "" | "/")
            {
                return Err(config_error(format!(
                    "{source} proxy must be auto, direct, or an http:// or socks5h:// endpoint"
                )));
            }
            Ok(ProxyPolicy::Explicit(url.to_string()))
        }
    }
}

/// Parses a nonnegative integer, allowing environment strings where specified.
fn parse_unsigned(
    value: &Value,
    field: &str,
    source: Source,
    allow_zero: bool,
) -> Result<usize, LabeledError> {
    let parsed = match value {
        Value::Int { val, .. } if *val >= 0 => usize::try_from(*val).ok(),
        Value::String { val, .. }
            if source == Source::Environment
                && !val.is_empty()
                && val.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            val.parse::<usize>().ok()
        }
        _ => None,
    };
    parsed
        .filter(|number| allow_zero || *number > 0)
        .ok_or_else(|| {
            config_error(format!(
                "{source} {field} must be {} integer",
                if allow_zero {
                    "a nonnegative"
                } else {
                    "a positive"
                }
            ))
        })
}

/// Converts a strictly positive integer to a nonzero table or cache bound.
fn parse_positive_usize(
    value: &Value,
    field: &str,
    source: Source,
) -> Result<NonZeroUsize, LabeledError> {
    NonZeroUsize::new(parse_unsigned(value, field, source, false)?)
        .ok_or_else(|| config_error(format!("{source} {field} must be positive")))
}

/// Resolves one cache limit independently of the other nested limit.
fn cache_limit(
    sources: &ConfigSources,
    record: Option<&Record>,
    key: &str,
    default: usize,
) -> Result<NonZeroUsize, LabeledError> {
    record
        .and_then(|record| record.get(key).map(|value| (Source::Plugin, value)))
        .or_else(|| {
            file_cache_value(sources.local.as_ref(), key).map(|value| (Source::Local, value))
        })
        .or_else(|| file_cache_value(sources.user.as_ref(), key).map(|value| (Source::User, value)))
        .map(|(source, value)| parse_positive_usize(value, &format!("cache.{key}"), source))
        .transpose()
        .map(|selected| {
            selected.unwrap_or_else(|| NonZeroUsize::new(default).expect("nonzero cache default"))
        })
}

/// Finds one nested file cache field while allowing another field to fall through.
fn file_cache_value<'a>(file: Option<&'a TomlSettings>, key: &str) -> Option<&'a Value> {
    match file?.values.get("cache") {
        Some(Value::Record { val, .. }) => val.get(key),
        _ => None,
    }
}

/// Produces an error without printing selected values or credentials.
fn config_error(message: impl Into<String>) -> LabeledError {
    LabeledError::new(message).with_code("jev::configuration")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use nu_protocol::{Record, Value};

    use super::{
        ConfigScope, ConfigSources, ProxyPolicy, parse_api_key, read_toml, require_api_key,
        resolve, resolve_models,
    };

    /// Owns an isolated test directory and removes it after each fixture.
    struct Fixture(PathBuf);

    impl Fixture {
        /// Creates a unique directory without modifying the process working directory.
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "jev-config-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create fixture directory");
            Self(path)
        }

        /// Writes a private TOML fixture and returns its absolute path.
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).expect("write TOML fixture");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                    .expect("set private permissions");
            }
            path
        }
    }

    impl Drop for Fixture {
        /// Removes only this fixture's isolated test directory.
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove fixture directory");
        }
    }

    /// Checks bounded reading, missing-file policy, syntax redaction, and schema keys.
    #[test]
    fn reads_bounded_files_and_redacts_parse_errors() {
        let fixture = Fixture::new();
        let missing = fixture.0.join("missing.toml");
        assert!(
            read_toml(&missing, false, false, "local")
                .unwrap()
                .is_none()
        );
        assert!(read_toml(&missing, true, false, "local").is_err());
        let empty = fixture.write("empty.toml", "");
        assert!(read_toml(&empty, true, false, "local").unwrap().is_some());
        let malformed = fixture.write("bad.toml", "api_key = 'SECRET_DO_NOT_SHOW' trailing");
        let message = read_toml(&malformed, true, false, "local")
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("line 1"));
        assert!(!message.contains("SECRET_DO_NOT_SHOW"));
        let unknown = fixture.write("unknown.toml", "unexpected = 1");
        assert!(read_toml(&unknown, true, false, "local").is_err());
        let large = fixture.write("large.toml", &" ".repeat(65_537));
        assert!(read_toml(&large, true, false, "local").is_err());
    }

    /// Composes partial files field by field and never falls back after invalid selection.
    #[test]
    fn resolves_file_overlays_and_key_priority() {
        let fixture = Fixture::new();
        let user_path = fixture.write("user.toml", "api_key = 'user-secret'\nmodel = 'user-model'\ntimeout_ms = 4000\n[cache]\nmax_entries = 9");
        let local_path = fixture.write(
            "local.toml",
            "jobs = 7\napi_key = 'local-secret'\n[cache]\nmax_approx_bytes = 12345",
        );
        let mut sources = ConfigSources {
            user: read_toml(&user_path, true, false, "user").unwrap(),
            local: read_toml(&local_path, true, true, "local").unwrap(),
            ..ConfigSources::default()
        };
        let config = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(config.model, "user-model");
        assert_eq!(config.timeout.as_millis(), 4000);
        assert_eq!(config.jobs.unwrap().get(), 7);
        assert_eq!(config.cache.unwrap().max_entries.get(), 9);
        assert_eq!(config.cache.unwrap().max_approx_bytes.get(), 12345);
        assert_eq!(require_api_key(&sources).unwrap().as_str(), "local-secret");
        sources.key = Some(Value::test_string("env-secret"));
        assert_eq!(require_api_key(&sources).unwrap().as_str(), "env-secret");
        sources.key = Some(Value::test_string(""));
        assert!(require_api_key(&sources).is_err());
        sources.key = None;
        sources.flags.insert("jobs", Value::test_int(0));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
    }

    /// Protects implicit local files from redirecting authenticated traffic.
    #[test]
    fn implicit_local_transport_is_rejected() {
        let fixture = Fixture::new();
        let path = fixture.write(
            "redirect.toml",
            "base_url = 'http://evil.invalid'\napi_key = 'secret'",
        );
        let message = read_toml(&path, true, true, "local")
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("base_url"));
        assert!(!message.contains("evil.invalid"));
        assert!(read_toml(&path, true, false, "local").is_ok());
    }

    /// Notices edits at the next read while earlier snapshots remain immutable.
    #[test]
    fn file_edits_do_not_change_existing_snapshots() {
        let fixture = Fixture::new();
        let path = fixture.write("local.toml", "model = 'first'");
        let first = ConfigSources {
            local: read_toml(&path, true, true, "local").unwrap(),
            ..ConfigSources::default()
        };
        fixture.write("local.toml", "model = 'second'");
        let second = ConfigSources {
            local: read_toml(&path, true, true, "local").unwrap(),
            ..ConfigSources::default()
        };
        assert_eq!(resolve(&first, ConfigScope::Single).unwrap().model, "first");
        assert_eq!(
            resolve(&second, ConfigScope::Single).unwrap().model,
            "second"
        );
    }

    /// Refuses live dispatch when any loaded key-bearing file is group-readable.
    #[cfg(unix)]
    #[test]
    fn key_files_must_be_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let path = fixture.write("key.toml", "api_key = 'secret'");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let sources = ConfigSources {
            local: read_toml(&path, true, true, "local").unwrap(),
            key: Some(Value::test_string("env-secret")),
            ..ConfigSources::default()
        };
        let message = require_api_key(&sources).err().unwrap().to_string();
        assert!(message.contains("owner-only"));
        assert!(!message.contains("secret"));
    }

    /// Constructs a Nu record for resolver fixtures.
    fn record(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        let mut result = Record::new();
        for (name, value) in fields {
            result.push(name, value);
        }
        Value::test_record(result)
    }

    /// Resolves defaults and independently defaulted table cache limits.
    #[test]
    fn resolves_defaults_and_partial_cache() {
        let defaults = resolve(&ConfigSources::default(), ConfigScope::Table).unwrap();
        assert_eq!(defaults.model, "jev-latest");
        assert_eq!(defaults.timeout.as_secs(), 30);
        assert_eq!(defaults.jobs.unwrap().get(), 16);
        assert_eq!(defaults.retries, 3);
        assert_eq!(defaults.cache.unwrap().max_approx_bytes.get(), 16_777_216);

        let sources = ConfigSources {
            plugin: Some(record([(
                "cache",
                record([("max_entries", Value::test_int(64))]),
            )])),
            ..ConfigSources::default()
        };
        let config = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(config.cache.unwrap().max_entries.get(), 64);
        assert_eq!(config.cache.unwrap().max_approx_bytes.get(), 16_777_216);
    }

    /// Applies flag, config, environment, and default precedence per setting.
    #[test]
    fn resolves_independent_priority() {
        let mut sources = ConfigSources {
            plugin: Some(record([
                ("model", Value::test_string("config-model")),
                ("timeout", Value::test_duration(2_000_000_000)),
            ])),
            ..ConfigSources::default()
        };
        sources
            .flags
            .insert("model", Value::test_string("flag-model"));
        sources
            .env
            .insert("NU_PLUGIN_JEV_MODEL", Value::test_string("env-model"));
        sources
            .env
            .insert("NU_PLUGIN_JEV_TIMEOUT_MS", Value::test_string("5000"));
        sources
            .env
            .insert("NU_PLUGIN_JEV_JOBS", Value::test_string("8"));
        let config = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(config.model, "flag-model");
        assert_eq!(config.timeout.as_secs(), 2);
        assert_eq!(config.jobs.unwrap().get(), 8);
        sources.plugin = None;
        let config = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(config.timeout.as_secs(), 5);
    }

    /// Accepts only plugin-scoped setting names while retaining the service key separately.
    #[test]
    fn ignores_replaced_environment_names() {
        let mut sources = ConfigSources::default();
        for (name, value) in [
            ("TYPESAFE_MODEL", "old-model"),
            ("TYPESAFE_BASE_URL", "http://localhost:1"),
            ("TYPESAFE_TIMEOUT_MS", "1"),
            ("TYPESAFE_JOBS", "1"),
            ("TYPESAFE_RETRIES", "0"),
            ("JEV_PROXY", "direct"),
        ] {
            sources.env.insert(name, Value::test_string(value));
        }
        let defaults = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(defaults.model, "jev-latest");
        assert_eq!(defaults.base_url.as_str(), "https://api.typesafe.ai/");
        assert_eq!(defaults.timeout.as_secs(), 30);
        assert_eq!(defaults.jobs.unwrap().get(), 16);
        assert_eq!(defaults.retries, 3);
        assert_eq!(defaults.proxy, ProxyPolicy::Auto);

        for (name, value) in [
            ("NU_PLUGIN_JEV_MODEL", "new-model"),
            ("NU_PLUGIN_JEV_BASE_URL", "http://localhost:2"),
            ("NU_PLUGIN_JEV_TIMEOUT_MS", "5000"),
            ("NU_PLUGIN_JEV_JOBS", "4"),
            ("NU_PLUGIN_JEV_RETRIES", "2"),
            ("NU_PLUGIN_JEV_PROXY", "direct"),
        ] {
            sources.env.insert(name, Value::test_string(value));
        }
        let config = resolve(&sources, ConfigScope::Table).unwrap();
        assert_eq!(config.model, "new-model");
        assert_eq!(config.base_url.as_str(), "http://localhost:2/");
        assert_eq!(config.timeout.as_secs(), 5);
        assert_eq!(config.jobs.unwrap().get(), 4);
        assert_eq!(config.retries, 2);
        assert_eq!(config.proxy, ProxyPolicy::Direct);
    }

    /// Rejects an invalid higher-priority setting rather than falling back.
    #[test]
    fn rejects_invalid_selected_values() {
        let mut sources = ConfigSources::default();
        sources.flags.insert("jobs", Value::test_int(0));
        sources
            .env
            .insert("NU_PLUGIN_JEV_JOBS", Value::test_string("8"));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
        sources.flags.clear();
        sources
            .env
            .insert("NU_PLUGIN_JEV_JOBS", Value::test_string("-1"));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
        sources.env.remove("NU_PLUGIN_JEV_JOBS");
        sources.plugin = Some(record([(
            "cache",
            record([("max_approx_bytes", Value::test_int(-1))]),
        )]));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
        sources.plugin = Some(record([(
            "cache",
            record([("max_entries", Value::test_string("many"))]),
        )]));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
        sources.plugin = Some(record([("timeout", Value::test_duration(0))]));
        assert!(resolve(&sources, ConfigScope::Single).is_err());
        sources.plugin = Some(record([("retries", Value::test_int(-1))]));
        assert!(resolve(&sources, ConfigScope::Single).is_err());
        sources.plugin = None;
        sources
            .env
            .insert("NU_PLUGIN_JEV_TIMEOUT_MS", Value::test_string("0"));
        assert!(resolve(&sources, ConfigScope::Table).is_err());
        sources.env.remove("NU_PLUGIN_JEV_TIMEOUT_MS");
        sources
            .env
            .insert("NU_PLUGIN_JEV_RETRIES", Value::test_string("0"));
        assert_eq!(resolve(&sources, ConfigScope::Single).unwrap().retries, 0);
    }

    /// Rejects invalid model settings and unsupported service roots.
    #[test]
    fn validates_service_roots_and_scope() {
        let mut sources = ConfigSources::default();
        sources
            .env
            .insert("NU_PLUGIN_JEV_MODEL", Value::test_int(1));
        assert!(resolve(&sources, ConfigScope::Single).is_err());
        sources.env.clear();
        for root in [
            "ftp://example.com",
            "https://user@example.com",
            "https://example.com/?q=1",
            "https://example.com/#fragment",
        ] {
            sources.flags.insert("base-url", Value::test_string(root));
            assert!(resolve(&sources, ConfigScope::Single).is_err());
        }
        sources
            .flags
            .insert("base-url", Value::test_string("http://localhost:8080/"));
        assert!(resolve(&sources, ConfigScope::Single).is_ok());
    }

    /// Requires a live caller key but never includes its content in an error.
    #[test]
    fn validates_live_key_without_exposing_it() {
        assert!(parse_api_key(None).is_err());
        assert!(parse_api_key(Some(Value::test_string(" "))).is_err());
        assert!(parse_api_key(Some(Value::test_int(1))).is_err());
        assert_eq!(
            parse_api_key(Some(Value::test_string("caller-secret")))
                .unwrap()
                .as_str(),
            "caller-secret"
        );
    }

    /// Resolves caller policy per invocation and rejects invalid selected values.
    #[test]
    fn resolves_proxy_priority_and_redacts_invalid_values() {
        let mut sources = ConfigSources::default();
        assert_eq!(
            resolve(&sources, ConfigScope::Single).unwrap().proxy,
            ProxyPolicy::Auto
        );
        sources
            .env
            .insert("NU_PLUGIN_JEV_PROXY", Value::test_string("direct"));
        assert_eq!(
            resolve(&sources, ConfigScope::Single).unwrap().proxy,
            ProxyPolicy::Direct
        );
        sources.plugin = Some(record([(
            "proxy",
            Value::test_string("http://user:secret@localhost:8080"),
        )]));
        assert_eq!(
            resolve(&sources, ConfigScope::Single).unwrap().proxy,
            ProxyPolicy::Explicit("http://user:secret@localhost:8080/".to_owned())
        );
        assert!(
            !format!("{:?}", resolve(&sources, ConfigScope::Single).unwrap()).contains("secret")
        );
        sources.plugin = Some(record([(
            "proxy",
            Value::test_string("socks5h://localhost:1080"),
        )]));
        assert!(matches!(
            resolve(&sources, ConfigScope::Table).unwrap().proxy,
            ProxyPolicy::Explicit(_)
        ));
        for invalid in [
            "socks5://user:secret@localhost",
            "http://user:secret@localhost/path",
            "not-a-proxy",
        ] {
            sources.plugin = Some(record([("proxy", Value::test_string(invalid))]));
            let error = resolve(&sources, ConfigScope::Single).unwrap_err();
            assert!(!error.to_string().contains("secret"));
        }
        sources.plugin = Some(record([("proxy", Value::test_int(1))]));
        assert!(resolve(&sources, ConfigScope::Single).is_err());
    }

    /// Resolves discovery transport independently of invalid evaluation-only settings.
    #[test]
    fn models_scope_uses_transport_precedence_only() {
        let fixture = Fixture::new();
        let user_path = fixture.write("models-user.toml", "api_key = 'user-key'\nbase_url = 'https://user.example/'\ntimeout_ms = 5000\nmodel = 7\njobs = -1\n[cache]\nmax_entries = -1");
        let local_path = fixture.write(
            "models-local.toml",
            "api_key = 'local-key'\nretries = 2\nmodel = 9",
        );
        let mut sources = ConfigSources {
            user: read_toml(&user_path, true, false, "user").unwrap(),
            local: read_toml(&local_path, true, true, "local").unwrap(),
            plugin: Some(record([
                ("model", Value::test_int(1)),
                ("jobs", Value::test_int(0)),
                ("cache", Value::test_int(1)),
                ("timeout", Value::test_duration(2_000_000_000)),
            ])),
            ..ConfigSources::default()
        };
        sources.env.insert(
            "NU_PLUGIN_JEV_BASE_URL",
            Value::test_string("https://env.example/"),
        );
        sources
            .env
            .insert("NU_PLUGIN_JEV_RETRIES", Value::test_string("4"));
        sources
            .flags
            .insert("base-url", Value::test_string("https://flag.example/"));
        let config = resolve_models(&sources).unwrap();
        assert_eq!(config.base_url.as_str(), "https://flag.example/");
        assert_eq!(config.timeout.as_secs(), 2);
        assert_eq!(config.retries, 4);
        assert_eq!(require_api_key(&sources).unwrap().as_str(), "local-key");
        sources.key = Some(Value::test_string("new-key"));
        assert_eq!(require_api_key(&sources).unwrap().as_str(), "new-key");
        sources.flags.insert("timeout", Value::test_duration(0));
        assert!(resolve_models(&sources).is_err());
    }

    /// Discovery retains implicit-file transport restrictions and key requirements.
    #[test]
    fn models_scope_keeps_transport_boundary() {
        let fixture = Fixture::new();
        let path = fixture.write("models-unsafe.toml", "proxy = 'http://localhost:1111'");
        assert!(read_toml(&path, true, true, "local").is_err());
        assert!(require_api_key(&ConfigSources::default()).is_err());
        assert_eq!(
            resolve_models(&ConfigSources::default()).unwrap().retries,
            3
        );
    }
}
