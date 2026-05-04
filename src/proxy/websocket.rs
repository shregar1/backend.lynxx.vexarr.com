use crate::error::GatewayError;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::FromRequestParts;
use axum::http::{Request, Response};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as TtMessage;

/// Bridges a client WebSocket upgrade to an upstream `ws://` server.
///
/// TLS upstreams (`wss://`) are not yet supported and return BadRequest.
pub async fn handle_websocket(
    req: Request<Body>,
    upstream_url: String,
) -> Result<Response<Body>, GatewayError> {
    let parsed = url::Url::parse(&upstream_url)
        .map_err(|e| GatewayError::BadRequest(format!("invalid upstream url: {e}")))?;

    let scheme = parsed.scheme();
    let ws_url = match scheme {
        "ws" => upstream_url.clone(),
        "http" => {
            // rewrite http -> ws for ergonomic config
            let mut s = upstream_url.clone();
            s.replace_range(0..4, "ws");
            s
        }
        "wss" | "https" => {
            return Err(GatewayError::BadRequest(
                "wss upstream not yet supported, use ws://".into(),
            ));
        }
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported websocket upstream scheme: {other}"
            )));
        }
    };

    // Run the axum WebSocketUpgrade extractor manually on the request parts.
    let (mut parts, _body) = req.into_parts();
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|e| GatewayError::BadRequest(format!("websocket upgrade rejected: {e}")))?;

    let response = upgrade.on_upgrade(move |client_socket| async move {
        if let Err(e) = bridge(client_socket, ws_url.clone()).await {
            tracing::warn!(error = %e, upstream = %ws_url, "websocket bridge ended with error");
        }
    });

    // axum::response::Response<Body> -> hyper Response<Body>
    Ok(response.into_response())
}

async fn bridge(client: WebSocket, upstream_url: String) -> Result<(), String> {
    let (upstream_ws, _resp) = tokio_tungstenite::connect_async(&upstream_url)
        .await
        .map_err(|e| format!("connect upstream ws: {e}"))?;

    let (mut client_sink, mut client_stream) = client.split();
    let (mut upstream_sink, mut upstream_stream) = upstream_ws.split();

    let c2u = async {
        while let Some(msg) = client_stream.next().await {
            match msg {
                Ok(m) => {
                    let upstream_msg = axum_to_tt(m);
                    if let Some(m) = upstream_msg {
                        if upstream_sink.send(m).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let _ = upstream_sink.close().await;
    };

    let u2c = async {
        while let Some(msg) = upstream_stream.next().await {
            match msg {
                Ok(m) => {
                    if let Some(am) = tt_to_axum(m) {
                        if client_sink.send(am).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let _ = client_sink.close().await;
    };

    tokio::select! {
        _ = c2u => {},
        _ = u2c => {},
    }
    Ok(())
}

fn axum_to_tt(msg: AxumMessage) -> Option<TtMessage> {
    Some(match msg {
        AxumMessage::Text(t) => TtMessage::Text(t),
        AxumMessage::Binary(b) => TtMessage::Binary(b),
        AxumMessage::Ping(p) => TtMessage::Ping(p),
        AxumMessage::Pong(p) => TtMessage::Pong(p),
        AxumMessage::Close(Some(cf)) => {
            TtMessage::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: cf.code.into(),
                reason: cf.reason,
            }))
        }
        AxumMessage::Close(None) => TtMessage::Close(None),
    })
}

fn tt_to_axum(msg: TtMessage) -> Option<AxumMessage> {
    Some(match msg {
        TtMessage::Text(t) => AxumMessage::Text(t),
        TtMessage::Binary(b) => AxumMessage::Binary(b),
        TtMessage::Ping(p) => AxumMessage::Ping(p),
        TtMessage::Pong(p) => AxumMessage::Pong(p),
        TtMessage::Close(Some(cf)) => AxumMessage::Close(Some(axum::extract::ws::CloseFrame {
            code: cf.code.into(),
            reason: cf.reason,
        })),
        TtMessage::Close(None) => AxumMessage::Close(None),
        TtMessage::Frame(_) => return None,
    })
}
