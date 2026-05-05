//! WebSocket flood: 500 concurrent WS sessions, 100 text frames/sec each.
//!
//! Spins up an embedded WS echo server (so we don't depend on a separate WS
//! upstream binary), points a service+route at it, then opens 500 connections
//! and pumps frames. Pass criteria: every connection stays open for the
//! entire test window — no drops, no upgrade timeouts.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::chaos::{fmt_count, AdminClient, Args, Report};

const SCENARIO: &str = "ws-flood";

pub async fn run(admin: &AdminClient, args: &Args) -> Result<Report> {
    let duration = args.duration_or(Duration::from_secs(60));

    // Spin up an embedded ws echo server on an ephemeral port.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(run_ws_echo(listener));

    let upstream_url = format!("http://{upstream_addr}");
    let svc_id = admin
        .create_service("chaos-ws", &upstream_url, 0, 5_000)
        .await?;
    let route_id = admin
        .create_route(svc_id, &["/chaos-ws"], &[], true)
        .await?;

    let proxy_ws = args
        .proxy_url
        .replacen("http://", "ws://", 1)
        .replacen("https://", "wss://", 1);
    let ws_url = format!("{}/chaos-ws", proxy_ws.trim_end_matches('/'));

    let connected = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(AtomicU64::new(0));
    let recv = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(500);
    for i in 0..500u64 {
        let url = ws_url.clone();
        let connected = connected.clone();
        let dropped = dropped.clone();
        let sent = sent.clone();
        let recv = recv.clone();
        handles.push(tokio::spawn(async move {
            let conn = match tokio_tungstenite::connect_async(&url).await {
                Ok((s, _)) => s,
                Err(_) => {
                    dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            connected.fetch_add(1, Ordering::Relaxed);
            let (mut tx, mut rx) = conn.split();

            let send_task = {
                let sent = sent.clone();
                tokio::spawn(async move {
                    let interval = Duration::from_millis(10); // 100 msg/sec
                    let total_msgs = (duration.as_millis() / interval.as_millis()) as u64;
                    for n in 0..total_msgs {
                        let msg = format!("c{i}-{n}");
                        if tx.send(Message::text(msg)).await.is_err() {
                            return false;
                        }
                        sent.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(interval).await;
                    }
                    let _ = tx.close().await;
                    true
                })
            };

            let recv_task = {
                let recv = recv.clone();
                tokio::spawn(async move {
                    while let Some(Ok(_msg)) = rx.next().await {
                        recv.fetch_add(1, Ordering::Relaxed);
                    }
                })
            };

            let send_ok = send_task.await.unwrap_or(false);
            let _ = recv_task.await;
            if !send_ok {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let _ = admin.delete_route(route_id).await;
    let _ = admin.delete_service(svc_id).await;

    let conn_n = connected.load(Ordering::Relaxed);
    let drop_n = dropped.load(Ordering::Relaxed);
    let sent_n = sent.load(Ordering::Relaxed);
    let recv_n = recv.load(Ordering::Relaxed);

    let pass = conn_n == 500 && drop_n == 0;
    let summary = format!(
        "{}/500 connected · {} dropped · {} sent · {} recv",
        conn_n,
        drop_n,
        fmt_count(sent_n),
        fmt_count(recv_n)
    );
    let r = if pass {
        Report::pass(SCENARIO, summary)
    } else {
        Report::fail(SCENARIO, summary)
    };
    Ok(r.detail(format!("ran {}s @ 100 msg/sec/conn", duration.as_secs())))
}

async fn run_ws_echo(listener: TcpListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let ws = match tokio_tungstenite::accept_async(stream).await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (mut tx, mut rx) = ws.split();
            while let Some(Ok(msg)) = rx.next().await {
                if msg.is_close() {
                    break;
                }
                if msg.is_text() || msg.is_binary() {
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        });
    }
}
