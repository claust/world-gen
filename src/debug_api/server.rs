use std::net::SocketAddr;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use tokio::sync::{broadcast, oneshot};
use tower_http::cors::{Any, CorsLayer};

use crate::debug_api::config::DebugApiConfig;
use crate::debug_api::types::{
    ApiErrorResponse, ApiStateResponse, CommandAcceptedResponse, CommandAppliedEvent,
    CommandRequest, HealthResponse, ServerEvent, TelemetrySnapshot, API_VERSION,
};

#[derive(Clone)]
struct HttpState {
    command_tx: mpsc::Sender<CommandRequest>,
    latest_telemetry: Arc<Mutex<Option<TelemetrySnapshot>>>,
    event_tx: broadcast::Sender<ServerEvent>,
}

pub struct DebugApiHandle {
    bind_addr: SocketAddr,
    command_rx: mpsc::Receiver<CommandRequest>,
    latest_telemetry: Arc<Mutex<Option<TelemetrySnapshot>>>,
    event_tx: broadcast::Sender<ServerEvent>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_thread: Option<std::thread::JoinHandle<()>>,
}

impl DebugApiHandle {
    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn drain_commands(&mut self) -> Vec<CommandRequest> {
        let mut commands = Vec::new();
        while let Ok(command) = self.command_rx.try_recv() {
            commands.push(command);
        }
        commands
    }

    pub fn publish_telemetry(&self, telemetry: TelemetrySnapshot) {
        if let Ok(mut slot) = self.latest_telemetry.lock() {
            *slot = Some(telemetry.clone());
        }
        let _ = self.event_tx.send(ServerEvent::Telemetry(telemetry));
    }

    pub fn publish_command_applied(&self, applied: CommandAppliedEvent) {
        let _ = self.event_tx.send(ServerEvent::CommandApplied(applied));
    }
}

impl Drop for DebugApiHandle {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start_debug_api(config: &DebugApiConfig) -> Result<Option<DebugApiHandle>> {
    if !config.enabled {
        return Ok(None);
    }

    let bind_addr: SocketAddr = config
        .bind_addr
        .parse()
        .with_context(|| format!("invalid debug api bind addr: {}", config.bind_addr))?;

    if !bind_addr.ip().is_loopback() {
        bail!("debug api must bind to loopback; got {}", bind_addr);
    }

    let (command_tx, command_rx) = mpsc::channel::<CommandRequest>();
    let (event_tx, _) = broadcast::channel::<ServerEvent>(512);
    let latest_telemetry = Arc::new(Mutex::new(None));
    let http_state = HttpState {
        command_tx,
        latest_telemetry: Arc::clone(&latest_telemetry),
        event_tx: event_tx.clone(),
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (startup_tx, startup_rx) = mpsc::channel::<Result<SocketAddr, String>>();

    let server_thread = std::thread::Builder::new()
        .name("debug-api".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    let _ = startup_tx.send(Err(format!("failed to create tokio runtime: {err}")));
                    return;
                }
            };

            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(bind_addr).await {
                    Ok(listener) => listener,
                    Err(err) => {
                        let _ = startup_tx.send(Err(format!("failed to bind debug api: {err}")));
                        return;
                    }
                };

                let local_addr = match listener.local_addr() {
                    Ok(addr) => addr,
                    Err(err) => {
                        let _ = startup_tx
                            .send(Err(format!("failed to read local bind address: {err}")));
                        return;
                    }
                };

                let app = Router::new()
                    .route("/health", get(health_handler))
                    .route("/api/state", get(state_handler))
                    .route("/api/command", post(command_handler))
                    .route("/ws", get(ws_handler))
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods(Any)
                            .allow_headers(Any),
                    )
                    .layer(axum::extract::DefaultBodyLimit::max(8 * 1024))
                    .with_state(http_state);

                let _ = startup_tx.send(Ok(local_addr));

                let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                });

                if let Err(err) = server.await {
                    eprintln!("debug api server stopped with error: {err}");
                }
            });
        })
        .context("failed to spawn debug api thread")?;

    let startup_result = startup_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| anyhow!("timed out waiting for debug api startup"))?;
    let bound_addr = startup_result.map_err(|msg| anyhow!(msg))?;

    Ok(Some(DebugApiHandle {
        bind_addr: bound_addr,
        command_rx,
        latest_telemetry,
        event_tx,
        shutdown_tx: Some(shutdown_tx),
        server_thread: Some(server_thread),
    }))
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        api_version: API_VERSION.to_string(),
        debug_api_enabled: true,
    })
}

async fn state_handler(State(state): State<HttpState>) -> Json<ApiStateResponse> {
    let latest = state
        .latest_telemetry
        .lock()
        .ok()
        .and_then(|snapshot| snapshot.clone());

    Json(ApiStateResponse {
        api_version: API_VERSION.to_string(),
        telemetry: latest,
    })
}

async fn command_handler(
    State(state): State<HttpState>,
    Json(request): Json<CommandRequest>,
) -> Result<(StatusCode, Json<CommandAcceptedResponse>), (StatusCode, Json<ApiErrorResponse>)> {
    if request.id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiErrorResponse {
                api_version: API_VERSION.to_string(),
                error: "invalid_command".to_string(),
                message: "command id cannot be empty".to_string(),
            }),
        ));
    }

    if state.command_tx.send(request.clone()).is_err() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiErrorResponse {
                api_version: API_VERSION.to_string(),
                error: "command_channel_closed".to_string(),
                message: "game loop is not receiving commands".to_string(),
            }),
        ));
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(CommandAcceptedResponse {
            api_version: API_VERSION.to_string(),
            id: request.id,
            status: "accepted".to_string(),
        }),
    ))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<HttpState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_client(socket, state))
}

async fn ws_client(mut socket: WebSocket, state: HttpState) {
    let mut event_rx = state.event_tx.subscribe();

    if let Some(initial) = state
        .latest_telemetry
        .lock()
        .ok()
        .and_then(|snapshot| snapshot.clone())
    {
        let event = ServerEvent::Telemetry(initial);
        if send_ws_event(&mut socket, &event).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        if send_ws_event(&mut socket, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
}

async fn send_ws_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<()> {
    let payload = serde_json::to_string(event).context("failed to serialize ws event")?;
    socket
        .send(Message::Text(payload))
        .await
        .context("failed to send ws event")
}
