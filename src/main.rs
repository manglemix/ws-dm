use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::Response,
    routing::any,
};
use axum_server::tls_rustls::RustlsConfig;
use dashmap::{DashMap, Entry};
use tokio::sync::oneshot::{self, Sender};

#[axum::debug_handler]
async fn create_room(
    ws: WebSocketUpgrade,
    Path(room_id): Path<String>,
    State(ws_sender_map): State<&'static DashMap<String, Sender<WebSocket>>>,
) -> Response {
    match ws_sender_map.entry(room_id.clone()) {
        Entry::Occupied(_) => Response::builder()
            .status(StatusCode::CONFLICT)
            .body(Default::default())
            .unwrap(),
        Entry::Vacant(vacant_entry) => ws.on_upgrade(move |mut ws| async move {
            let (sender, receiver) = oneshot::channel();
            vacant_entry.insert(sender);
            let result = tokio::select! {
                result = receiver => result,
                _ = async {
                    loop {
                        let Some(Ok(_)) = ws.recv().await else {
                            break;
                        };
                    }
                } => {
                    ws_sender_map.remove(&room_id);
                    return;
                }
            };
            let Ok(mut peer_ws) = result else {
                ws_sender_map.remove(&room_id);
                return;
            };
            loop {
                tokio::select! {
                    option = ws.recv() => {
                        let Some(Ok(msg)) = option else {
                            break;
                        };
                        if peer_ws.send(msg).await.is_err() {
                            break;
                        }
                    }
                    option = peer_ws.recv() => {
                        let Some(Ok(msg)) = option else {
                            break;
                        };
                        if ws.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }),
    }
}

#[axum::debug_handler]
async fn join_room(
    ws: WebSocketUpgrade,
    Path(room_id): Path<String>,
    State(ws_sender_map): State<&'static DashMap<String, Sender<WebSocket>>>,
) -> Response {
    match ws_sender_map.remove(&room_id) {
        Some((_, sender)) => ws.on_upgrade(move |ws| async move {
            let _ = sender.send(ws);
        }),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Default::default())
            .unwrap(),
    }
}

#[tokio::main]
async fn main() {
    // build our application with a single route
    let app = Router::new()
        .route("/create/{room_id}", any(create_room))
        .route("/join/{room_id}", any(join_room))
        .with_state(Box::leak(Box::new(DashMap::default())))
        .layer(tower_http::compression::CompressionLayer::new());

    let mut args = std::env::args().skip(1);
    let cert_path = args.next();

    if let Some(cert_path) = cert_path {
        let key_path = args.next().expect("Expected key path after cert path");

        let config = RustlsConfig::from_pem_file(cert_path, key_path)
            .await
            .unwrap();

        axum_server::tls_rustls::bind_rustls("0.0.0.0:443".parse().unwrap(), config)
            .serve(app.into_make_service())
            .await
            .unwrap();
        return;
    }

    let listener = tokio::net::TcpListener::bind("0.0.0.0:80").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
