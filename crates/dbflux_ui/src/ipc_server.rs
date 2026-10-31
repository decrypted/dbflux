use crate::ui::views::workspace::Workspace;
use dbflux_ipc::{
    APP_CONTROL_VERSION, framing,
    protocol::{AppControlRequest, AppControlResponse, IpcMessage, IpcResponse},
    tokens_match,
};
use gpui::*;
use interprocess::local_socket::{
    Listener as IpcListener, Stream as IpcStream, traits::Listener as _, traits::Stream as _,
};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Accepts are served one at a time, so a client that connects and goes
/// silent must not hold the channel for longer than this.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct IpcServer;

enum IpcCommand {
    OpenScript { path: PathBuf },
    Focus,
}

impl IpcServer {
    pub fn start_with_listener(
        listener: IpcListener,
        workspace: Entity<Workspace>,
        window: AnyWindowHandle,
        auth_token: String,
        cx: &mut App,
    ) {
        let (cmd_tx, cmd_rx): (Sender<IpcCommand>, Receiver<IpcCommand>) = mpsc::channel();

        thread::spawn(move || {
            accept_loop(listener, cmd_tx, auth_token);
        });

        cx.spawn(async move |cx| {
            process_commands(cmd_rx, workspace, window, cx.clone()).await;
        })
        .detach();
    }
}

fn accept_loop(listener: IpcListener, cmd_tx: Sender<IpcCommand>, auth_token: String) {
    loop {
        match listener.accept() {
            Ok(stream) => {
                if let Err(e) = dbflux_ipc::verify_peer(&stream) {
                    log::warn!("IPC client rejected: {}", e);
                    continue;
                }

                if let Err(e) = handle_connection(stream, &cmd_tx, &auth_token) {
                    log::warn!("IPC connection error: {}", e);
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                log::warn!("IPC accept error: {}", e);
                break;
            }
        }
    }
}

fn handle_connection(
    mut stream: IpcStream,
    cmd_tx: &Sender<IpcCommand>,
    expected_auth_token: &str,
) -> io::Result<()> {
    // Windows named pipes have no timeouts and report an error; elsewhere a
    // failure means a silent client can hold the channel, so say so.
    if let Err(error) = stream.set_recv_timeout(Some(REQUEST_READ_TIMEOUT)) {
        if cfg!(windows) {
            log::debug!("IPC read timeout not applied: {error}");
        } else {
            log::warn!("IPC read timeout not applied: {error}");
        }
    }
    let request: AppControlRequest = framing::recv_msg(&mut stream)?;
    let request_id = request.request_id;

    if !request
        .protocol_version
        .is_compatible_with(APP_CONTROL_VERSION)
    {
        let response = AppControlResponse::ok(
            request_id,
            IpcResponse::Error {
                message: "incompatible app-control protocol version".to_string(),
            },
        );
        framing::send_msg(&mut stream, &response)?;
        return Ok(());
    }

    if !tokens_match(request.auth_token.as_deref(), expected_auth_token) {
        let response = AppControlResponse::ok(
            request_id,
            IpcResponse::Error {
                message: "unauthorized app-control request".to_string(),
            },
        );
        framing::send_msg(&mut stream, &response)?;
        return Ok(());
    }

    let response_body = match request.body {
        IpcMessage::Ping => IpcResponse::Pong {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        IpcMessage::OpenScript { path } => {
            let path = path.canonicalize().unwrap_or(path);
            if cmd_tx.send(IpcCommand::OpenScript { path }).is_ok() {
                IpcResponse::Ok
            } else {
                IpcResponse::Error {
                    message: "failed to send command".into(),
                }
            }
        }
        IpcMessage::Focus => {
            if cmd_tx.send(IpcCommand::Focus).is_ok() {
                IpcResponse::Ok
            } else {
                IpcResponse::Error {
                    message: "failed to send command".into(),
                }
            }
        }
    };

    let response = AppControlResponse::ok(request_id, response_body);

    framing::send_msg(&mut stream, &response)?;
    Ok(())
}

async fn process_commands(
    cmd_rx: Receiver<IpcCommand>,
    workspace: Entity<Workspace>,
    window: AnyWindowHandle,
    cx: AsyncApp,
) {
    loop {
        match cmd_rx.try_recv() {
            Ok(cmd) => {
                cx.update(|cx| match cmd {
                    IpcCommand::OpenScript { path } => {
                        workspace.update(cx, |ws, cx| {
                            ws.open_script_from_path(path, cx);
                        });
                    }
                    IpcCommand::Focus => {
                        if let Err(e) = window.update(cx, |_root, window, _cx| {
                            window.activate_window();
                        }) {
                            log::warn!("Failed to focus main window via IPC: {:?}", e);
                        }
                    }
                });
            }
            Err(mpsc::TryRecvError::Empty) => {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                break;
            }
        }
    }
}
