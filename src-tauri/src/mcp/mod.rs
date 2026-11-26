//! Управление локальным MCP и отдельными настройками его подключения.

mod stdio;
mod tools;
mod transport;

use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use km_shared::{GenMode, MCP_PORT, McpModelDefaults, McpStatus, Provider};
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::{net::TcpListener, sync::Mutex, sync::oneshot, task::JoinHandle};
use uuid::Uuid;

use crate::state::AppState;

pub(crate) const SKILL: &str = include_str!("../../../skills/media-studio/SKILL.md");
pub(super) const INSTRUCTIONS: &str = "Koteyye Media Studio uses the active provider and saves media in the configured folder. Read koteyye://skills/media-studio/SKILL.md or call get_studio_skill for the workflow. Use studio_status for a read-only connection check. Generation may incur charges.";
pub(super) const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

pub use stdio::run_stdio;

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
struct Config {
    enabled: bool,
    token: Option<String>,
    #[serde(default, skip_serializing_if = "McpModelDefaults::is_empty")]
    model_defaults: McpModelDefaults,
}

struct Running {
    token: Arc<RwLock<Option<String>>>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<io::Result<()>>,
}

impl Running {
    fn spawn(listener: TcpListener, state: &Arc<AppState>, secret: String) -> Self {
        let token = Arc::new(RwLock::new(Some(secret)));
        let (stop, receiver) = oneshot::channel();
        let task = tokio::spawn(transport::serve(
            listener,
            Arc::clone(state),
            Arc::clone(&token),
            receiver,
        ));
        Self { token, stop, task }
    }
}

#[derive(Default)]
struct Lifecycle {
    config: Config,
    server: Option<Running>,
    error: Option<String>,
}

pub(crate) struct Service {
    path: PathBuf,
    port: u16,
    lifecycle: Mutex<Lifecycle>,
}

impl Service {
    pub(crate) fn load(path: PathBuf) -> Self {
        let lifecycle = match read_config(&path) {
            Ok(config) => Lifecycle {
                config,
                ..Default::default()
            },
            Err(err) => Lifecycle {
                error: Some(format!("Failed to read MCP settings: {err}")),
                ..Default::default()
            },
        };
        Self {
            path,
            port: MCP_PORT,
            lifecycle: Mutex::new(lifecycle),
        }
    }

    pub(crate) async fn restore(&self, state: &Arc<AppState>) {
        let mut lifecycle = self.lifecycle.lock().await;
        if lifecycle.config.enabled {
            self.start(state, &mut lifecycle).await;
        }
    }

    fn snapshot(&self, lifecycle: &Lifecycle) -> McpStatus {
        let running = lifecycle
            .server
            .as_ref()
            .is_some_and(|server| !server.task.is_finished());
        let error = lifecycle.error.clone().or_else(|| {
            lifecycle
                .server
                .as_ref()
                .filter(|_| !running)
                .map(|_| "MCP server stopped".to_owned())
        });
        McpStatus {
            enabled: lifecycle.config.enabled,
            running,
            endpoint: endpoint(self.port),
            command: std::env::current_exe()
                .ok()
                .and_then(|path| path.into_os_string().into_string().ok()),
            token: lifecycle
                .config
                .enabled
                .then(|| lifecycle.config.token.clone())
                .flatten(),
            error,
            model_defaults: lifecycle.config.model_defaults.clone(),
        }
    }

    async fn start(&self, state: &Arc<AppState>, lifecycle: &mut Lifecycle) {
        if lifecycle
            .server
            .as_ref()
            .is_some_and(|server| !server.task.is_finished())
        {
            return;
        }
        let Some(secret) = lifecycle.config.token.clone() else {
            lifecycle.error = Some("MCP token is missing".to_owned());
            return;
        };
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let listener = match TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(err) => {
                lifecycle.error = Some(format!("Could not listen on {addr}: {err}"));
                return;
            }
        };
        lifecycle.server = Some(Running::spawn(listener, state, secret));
        lifecycle.error = None;
    }

    async fn set_enabled(&self, state: &Arc<AppState>, enabled: bool) -> Result<McpStatus, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.config = update_config(&self.path, |config| {
            config.enabled = enabled;
            if enabled && config.token.is_none() {
                config.token = Some(new_token());
            }
        })
        .map_err(|err| err.to_string())?;
        if enabled {
            self.start(state, &mut lifecycle).await;
        } else {
            if let Some(Running {
                token,
                stop,
                mut task,
            }) = lifecycle.server.take()
            {
                *token.write().unwrap_or_else(|err| err.into_inner()) = None;
                let _ = stop.send(());
                if tokio::time::timeout(Duration::from_secs(1), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            }
            lifecycle.error = None;
        }
        Ok(self.snapshot(&lifecycle))
    }

    async fn rotate(&self) -> Result<McpStatus, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        let Some(server) = lifecycle
            .server
            .as_ref()
            .filter(|server| !server.task.is_finished())
        else {
            return Err("Start the MCP server in this instance before rotating its token".into());
        };
        let secret = new_token();
        let config = update_config(&self.path, |config| config.token = Some(secret.clone()))
            .map_err(|err| err.to_string())?;
        *server.token.write().unwrap_or_else(|err| err.into_inner()) = Some(secret);
        lifecycle.config = config;
        Ok(self.snapshot(&lifecycle))
    }

    pub(super) async fn model_defaults(&self) -> McpModelDefaults {
        if let Ok(config) = read_config(&self.path) {
            return config.model_defaults;
        }
        self.lifecycle.lock().await.config.model_defaults.clone()
    }

    pub(super) async fn set_model_default(
        &self,
        provider: Provider,
        mode: GenMode,
        model: Option<String>,
    ) -> Result<McpStatus, String> {
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.config = update_config(&self.path, |config| match model {
            Some(model) => {
                config
                    .model_defaults
                    .entry(provider)
                    .or_default()
                    .insert(mode, model);
            }
            None => {
                if let Some(defaults) = config.model_defaults.get_mut(&provider) {
                    defaults.remove(&mode);
                    if defaults.is_empty() {
                        config.model_defaults.remove(&provider);
                    }
                }
            }
        })
        .map_err(|err| err.to_string())?;
        Ok(self.snapshot(&lifecycle))
    }

    /// Настройка и приём задачи сериализованы. HTTP к провайдеру идёт после лока.
    pub(crate) async fn with_model_policy<T>(
        &self,
        provider: Provider,
        mode: GenMode,
        model: &str,
        model_was_omitted: bool,
        begin: impl FnOnce() -> T,
    ) -> Result<T, String> {
        let _lifecycle = self.lifecycle.lock().await;
        let lock = config_lock(&self.path).map_err(|err| err.to_string())?;
        lock.lock_shared().map_err(|err| err.to_string())?;
        let config = read_config(&self.path).map_err(|err| err.to_string())?;
        let fixed = config
            .model_defaults
            .get(&provider)
            .and_then(|defaults| defaults.get(&mode));
        let allowed = match fixed {
            Some(fixed) => fixed == model,
            None => !model_was_omitted,
        };
        if !allowed {
            return Err("The Studio MCP model policy changed. Check studio_status before starting the generation.".into());
        }
        Ok(begin())
    }
}

pub(crate) async fn serve_headless(
    state: &Arc<AppState>,
    stop: impl Future<Output = ()> + Send,
    ready: impl FnOnce(&McpStatus),
) -> Result<(), String> {
    let service = &state.mcp;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, service.port));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|err| format!("Could not listen on {addr}: {err}"))?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();

    let (secret, status) = {
        let mut lifecycle = service.lifecycle.lock().await;
        lifecycle.config = update_config(&service.path, |config| {
            config.token.get_or_insert_with(new_token);
        })
        .map_err(|err| format!("Failed to load MCP settings: {err}"))?;
        lifecycle.error = None;
        let secret = lifecycle.config.token.clone().expect("MCP token prepared");
        let mut status = service.snapshot(&lifecycle);
        status.running = true;
        status.endpoint = endpoint(port);
        status.token = Some(secret.clone());
        (secret, status)
    };

    let Running {
        token,
        stop: stop_server,
        mut task,
    } = Running::spawn(listener, state, secret);
    let recovery = tokio::spawn({
        let state = Arc::clone(state);
        async move { crate::commands::cmd_recover_pending(&state).await }
    });
    ready(&status);

    let completed = tokio::select! {
        completed = &mut task => Some(completed),
        () = stop => None,
    };
    *token.write().unwrap_or_else(|err| err.into_inner()) = None;
    let result = match completed {
        Some(completed) => completed,
        None => {
            let _ = stop_server.send(());
            task.await
        }
    };

    if let Err(err) = recovery
        .await
        .map_err(|err| err.to_string())
        .and_then(|result| result.map(|_| ()))
    {
        eprintln!("Startup recovery failed: {err}");
    }
    // После закрытия соединений новые задания не поступят. Дожидаемся
    // завершения уже принятых задач, включая запущенные в фоне через MCP.
    let _idle = state.recovery_lock().write().await;
    result
        .map_err(|err| format!("MCP server task failed: {err}"))?
        .map_err(|err| format!("MCP server failed: {err}"))
}

fn new_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

fn read_config(path: &Path) -> io::Result<Config> {
    let meta = match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        result => result?,
    };
    if !meta.is_file() || meta.len() > 16 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid MCP settings file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "MCP settings file must be private",
            ));
        }
    }
    let config: Config = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid MCP settings"))?;
    if config
        .token
        .as_ref()
        .is_some_and(|token| !valid_token(token))
        || (config.enabled && config.token.is_none())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid MCP token",
        ));
    }
    Ok(config)
}

fn write_config(path: &Path, config: &Config) -> io::Result<()> {
    let folder = path
        .parent()
        .ok_or_else(|| io::Error::other("MCP settings directory missing"))?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    serde_json::to_writer(&mut file, config)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|err| err.error)?;
    Ok(())
}

fn config_lock(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
}

fn update_config(path: &Path, update: impl FnOnce(&mut Config)) -> io::Result<Config> {
    let lock = config_lock(path)?;
    lock.lock()?;
    let previous = read_config(path)?;
    let mut config = previous.clone();
    update(&mut config);
    if config != previous {
        write_config(path, &config)?;
    }
    Ok(config)
}

fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

#[tauri::command]
pub(crate) async fn mcp_status(state: State<'_, Arc<AppState>>) -> Result<McpStatus, String> {
    let lifecycle = state.mcp.lifecycle.lock().await;
    Ok(state.mcp.snapshot(&lifecycle))
}

#[tauri::command]
pub(crate) async fn mcp_set_enabled(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<McpStatus, String> {
    state.mcp.set_enabled(state.inner(), enabled).await
}

#[tauri::command]
pub(crate) async fn mcp_rotate_token(state: State<'_, Arc<AppState>>) -> Result<McpStatus, String> {
    state.mcp.rotate().await
}

#[tauri::command]
pub(crate) async fn mcp_set_model_default(
    state: State<'_, Arc<AppState>>,
    provider: Provider,
    mode: GenMode,
    model: Option<String>,
) -> Result<McpStatus, String> {
    if let Some(id) = model.as_deref() {
        if id.trim().is_empty() || id.len() > 256 {
            return Err("Invalid model ID".into());
        }
        let catalog = crate::commands::cmd_fetch_models_for(&state, provider, mode.kind()).await?;
        if !catalog
            .iter()
            .any(|entry| entry.id == id && entry.supports_mode(mode))
        {
            return Err(
                "The selected model does not support this generation mode in the provider catalog"
                    .into(),
            );
        }
    }
    state.mcp.set_model_default(provider, mode, model).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::InMemoryKeyStore;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn headless_serves_saved_preferences_and_drains_the_generation_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        // Второе состояние успело загрузиться до создания токена терминалом.
        let gui = Service::load(path.clone());
        let mut state = AppState::new(
            &dir.path().join("history.db"),
            Box::new(InMemoryKeyStore::default()),
            "http://127.0.0.1:1",
            Arc::new(|_| {}),
        )
        .unwrap();
        state.mcp.port = 0;
        let state = Arc::new(state);
        let (stop, stopped) = oneshot::channel();
        let (ready, started) = oneshot::channel();
        let mut server = tokio::spawn({
            let state = Arc::clone(&state);
            async move {
                serve_headless(
                    &state,
                    async {
                        let _ = stopped.await;
                    },
                    |status| {
                        let _ = ready.send(status.clone());
                    },
                )
                .await
            }
        });
        let status = started.await.unwrap();
        assert!(status.running && !status.enabled);
        let token = status.token.unwrap();
        assert!(valid_token(&token));

        gui.set_model_default(
            Provider::CheaperInference,
            GenMode::TextToImage,
            Some("preferred-image".into()),
        )
        .await
        .unwrap();
        let config = read_config(&path).unwrap();
        assert_eq!(config.token.as_deref(), Some(token.as_str()));
        assert!(!config.enabled);
        assert_eq!(state.mcp.model_defaults().await, config.model_defaults);
        assert!(
            state
                .mcp
                .with_model_policy(
                    Provider::CheaperInference,
                    GenMode::TextToImage,
                    "different-model",
                    false,
                    || ()
                )
                .await
                .is_err()
        );

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result: Value = client
            .post(status.endpoint)
            .bearer_auth(&token)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(result["result"]["tools"].as_array().is_some());

        let generation = state.hold_generation().await.unwrap();
        stop.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut server)
                .await
                .is_err()
        );
        drop(generation);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!read_config(&path).unwrap().enabled);
    }

    #[tokio::test]
    async fn enable_rotate_disable_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::new(
                &dir.path().join("history.db"),
                Box::new(InMemoryKeyStore::default()),
                "http://127.0.0.1:1",
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        let path = dir.path().join("mcp.json");
        let mut service = Service::load(path.clone());
        service.port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(!service.snapshot(&*service.lifecycle.lock().await).enabled);
        assert!(service.model_defaults().await.is_empty());
        let configured = service
            .set_model_default(
                Provider::CheaperInference,
                GenMode::TextToImage,
                Some("preferred-image".into()),
            )
            .await
            .unwrap();
        assert!(!configured.enabled && !configured.running);
        let configured = service
            .set_model_default(
                Provider::OpenRouter,
                GenMode::TextToImage,
                Some("or/preferred-image".into()),
            )
            .await
            .unwrap();
        let first = service.set_enabled(&state, true).await.unwrap();
        assert!(first.enabled && first.running);
        assert_eq!(first.model_defaults, configured.model_defaults);
        let first_token = first.token.unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        let call = |token| {
            client
                .post(&first.endpoint)
                .bearer_auth(token)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .send()
        };
        let value: Value = call(first_token.clone())
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(value["result"]["tools"].as_array().is_some());
        assert_eq!(
            read_config(&path).unwrap().token.as_deref(),
            Some(first_token.as_str())
        );
        let next = service.rotate().await.unwrap();
        assert_eq!(next.model_defaults, configured.model_defaults);
        let next_token = next.token.unwrap();
        assert_ne!(first_token, next_token);
        assert_eq!(call(first_token).await.unwrap().status(), 401);
        assert_eq!(call(next_token.clone()).await.unwrap().status(), 200);
        assert!(read_config(&path).unwrap().enabled);

        // Закрытие процесса оставляет настройки включёнными.
        let Running { stop, task, .. } = service.lifecycle.lock().await.server.take().unwrap();
        let _ = stop.send(());
        task.await.unwrap().unwrap();
        let mut restored = Service::load(path.clone());
        restored.port = service.port;
        restored.restore(&state).await;
        assert_eq!(call(next_token.clone()).await.unwrap().status(), 200);
        assert!(restored.snapshot(&*restored.lifecycle.lock().await).running);
        let disabled = restored.set_enabled(&state, false).await.unwrap();
        assert!(!disabled.enabled && !disabled.running && disabled.token.is_none());
        assert_eq!(disabled.model_defaults, configured.model_defaults);
        assert!(!read_config(&path).unwrap().enabled);
        assert!(call(next_token).await.is_err());
        let automatic = restored
            .set_model_default(Provider::CheaperInference, GenMode::TextToImage, None)
            .await
            .unwrap();
        assert!(
            !automatic
                .model_defaults
                .contains_key(&Provider::CheaperInference)
        );
        assert_eq!(
            read_config(&path).unwrap().model_defaults,
            automatic.model_defaults
        );
        assert_eq!(
            automatic.model_defaults[&Provider::OpenRouter][&GenMode::TextToImage],
            "or/preferred-image",
        );
    }
}
