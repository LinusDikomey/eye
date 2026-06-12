mod lsp;
mod types;

use std::io::{BufRead, Read};

use lsp::Lsp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing_subscriber::{Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use types::Id;

use crate::types::TextDocumentSyncOptions;

#[derive(Debug)]
pub enum LspError {
    IO(std::io::Error),
}
impl From<std::io::Error> for LspError {
    fn from(value: std::io::Error) -> Self {
        Self::IO(value)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message<'a> {
    jsonrpc: &'a str,
    #[serde(flatten)]
    message: MessageType<'a>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum MessageType<'a> {
    #[serde(borrow)]
    Request(Request<'a>),
    #[serde(borrow)]
    Notification(Notification<'a>),
    Response(Response),
}

#[derive(Debug, Serialize, Deserialize)]
struct Request<'a> {
    id: Id,
    method: &'a str,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Response {
    Ok { id: Id, result: Value },
    Err { id: Id, error: ResponseError },
}

#[derive(Debug, Serialize, Deserialize)]
struct Notification<'a> {
    method: &'a str,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResponseError {
    code: i64,
    message: String,
    data: Value,
}
impl From<serde_json::Error> for ResponseError {
    fn from(value: serde_json::Error) -> Self {
        Self {
            code: ERROR_INVALID_REQUEST,
            message: format!("Failed to deserialize params: {value:?}"),
            data: Value::Null,
        }
    }
}

const ERROR_FAILED: i64 = -32803;
const ERROR_SERVER_NOT_INITIALIZED: i64 = -32002;
const ERROR_INVALID_REQUEST: i64 = -32600;

const JSONRPC: &str = "2.0";

#[allow(clippy::large_enum_variant)]
enum State {
    Ready,
    Running { lsp: Lsp },
    Shutdown,
}

impl State {
    fn handle(&mut self, message: Message) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            State::Ready => match message.message {
                MessageType::Request(request) if request.method == "initialize" => {
                    match serde_json::from_value::<types::Initialize>(request.params) {
                        Ok(initialize) => {
                            *self = State::Running {
                                lsp: Lsp::new(initialize),
                            };
                            send_response(
                                request.id,
                                &types::InitializeResult {
                                    capabilities: types::ServerCapabilities {
                                        positionEncoding: "utf-8".to_owned(),
                                        hoverProvider: true,
                                        completionProvider: Some(types::CompletionOptions {
                                            triggerCharacters: vec![".".to_owned()],
                                            completionItem: None,
                                        }),
                                        textDocumentSync: Some(TextDocumentSyncOptions {
                                            openClose: true,
                                            change: Some(types::TextDocumentSyncKind::Incremental),
                                            save: true,
                                        }),
                                        definitionProvider: true,
                                        documentFormattingProvider: true,
                                    },
                                    server_info: types::ServerInfo {
                                        name: "eye-lsp".to_owned(),
                                        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                                    },
                                },
                            );
                        }
                        Err(err) => {
                            tracing::debug!("invalid initialize request, received: {err:?}");
                            send_err_response(
                                request.id,
                                ERROR_INVALID_REQUEST,
                                format!("invalid initialize request: {err:?}"),
                            );
                        }
                    }
                }
                MessageType::Request(request) => {
                    send_err_response(
                        request.id,
                        ERROR_SERVER_NOT_INITIALIZED,
                        "the lsp is not initialized yet".to_owned(),
                    );
                }
                _ => {}
            },
            State::Running { lsp } => match message.message {
                MessageType::Request(request) if request.method == "shutdown" => {
                    *self = State::Shutdown;
                    send(
                        serde_json::to_string(&Message {
                            jsonrpc: JSONRPC,
                            message: MessageType::Response(Response::Ok {
                                id: request.id,
                                result: Value::Null,
                            }),
                        })
                        .unwrap(),
                    );
                }
                MessageType::Request(request) => {
                    if request.method == "shutdown" {
                        *self = State::Shutdown;
                        return Ok(());
                    }
                    let response = match lsp.handle_request(request.method, request.params) {
                        Ok(result) => Response::Ok {
                            id: request.id,
                            result,
                        },
                        Err(error) => Response::Err {
                            id: request.id,
                            error,
                        },
                    };
                    send(
                        serde_json::to_string(&Message {
                            jsonrpc: JSONRPC,
                            message: MessageType::Response(response),
                        })
                        .unwrap(),
                    );
                }
                MessageType::Notification(notification) => {
                    match lsp.handle_notification(notification.method, notification.params) {
                        Ok(()) => {}
                        Err(err) => {
                            tracing::error!("Failed to handle notification: {err:?}");
                        }
                    }
                }
                MessageType::Response(_) => {}
            },
            State::Shutdown => match message.message {
                MessageType::Notification(notification) if notification.method == "exit" => {
                    std::process::exit(0);
                }
                MessageType::Request(request) => send_err_response(
                    request.id,
                    ERROR_INVALID_REQUEST,
                    "server was shutdown".to_owned(),
                ),
                _ => {}
            },
        }
        Ok(())
    }
}

pub fn run() {
    enable_tracing();

    tracing::debug!(
        "Starting lsp in {}",
        std::env::current_dir().unwrap().display()
    );
    if let Err(err) = message_loop() {
        tracing::error!("LSP failed: {err:?}");
    }
}

fn message_loop() -> Result<(), LspError> {
    let mut state = State::Ready;

    let mut stdin = std::io::stdin().lock();
    let mut buf = String::new();
    let mut content_buf = Vec::new();
    loop {
        buf.clear();
        stdin.read_line(&mut buf)?;
        buf.make_ascii_lowercase();
        let Some(len): Option<usize> = buf
            .strip_prefix("content-length: ")
            .and_then(|s| s.strip_suffix("\r\n"))
            .and_then(|s| s.parse().ok())
        else {
            tracing::debug!(target: "lsp", "invalid content-length header");
            continue;
        };
        buf.clear();
        stdin.read_line(&mut buf)?;
        if buf != "\r\n" {
            tracing::debug!("invalid line after content-length header");
            continue;
        }
        content_buf.clear();
        content_buf.resize(len, 0);
        stdin.read_exact(&mut content_buf)?;
        let message: Message = match serde_json::from_slice(&content_buf) {
            Ok(request) => request,
            Err(err) => {
                tracing::debug!(
                    "invalid request: {err:?}, request is: {}",
                    String::from_utf8_lossy(&content_buf)
                );
                continue;
            }
        };
        if let Err(err) = state.handle(message) {
            tracing::debug!("Failed to handle message: {err:?}");
        }
    }
}

fn enable_tracing() {
    let log_dir = std::env::home_dir().unwrap().join(".cache/eye/");
    _ = std::fs::create_dir_all(&log_dir);
    let file = log_dir.join("eye-lsp.log");
    let debug_file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .unwrap();

    tracing_subscriber::Registry::default()
        .with(
            fmt::layer()
                .compact()
                .with_ansi(false)
                .with_writer(debug_file)
                .with_filter(tracing_subscriber::filter::LevelFilter::from_level(
                    tracing::Level::INFO,
                )),
        )
        .init();
    tracing::info!("Eye LSP starting");
}

fn send_response<T: Serialize>(id: Id, result: &T) {
    let result = serde_json::to_value(result).unwrap();
    let s = serde_json::to_string(&Message {
        jsonrpc: "2.0",
        message: MessageType::Response(Response::Ok { id, result }),
    })
    .unwrap();
    send(s);
}

fn send_err_response(id: Id, code: i64, message: String) {
    send(
        serde_json::to_string(&Message {
            jsonrpc: "2.0",
            message: MessageType::Response(Response::Err {
                id,
                error: ResponseError {
                    code,
                    message,
                    data: Value::Null,
                },
            }),
        })
        .unwrap(),
    );
}

fn send(s: String) {
    print!("Content-Length: {}\r\n\r\n{s}\n", s.len())
}

fn send_notification<N: types::notification::ServerNotification>(notification: N) {
    let s = serde_json::to_string(&Message {
        jsonrpc: JSONRPC,
        message: MessageType::Notification(Notification {
            method: N::METHOD,
            params: serde_json::to_value(notification).unwrap(),
        }),
    })
    .unwrap();
    send(s);
}
