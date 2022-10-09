use std::net::SocketAddr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::{pin::Pin, task::Poll};

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use byteorder::{BigEndian, ByteOrder};
use futures::{Sink, Stream};
use pin_project::pin_project;
use sync_wrapper::SyncWrapper;
use tokio::{
    io::{copy_bidirectional, AsyncRead, AsyncWrite},
    net::TcpStream,
};

#[shuttle_service::main]
async fn axum() -> shuttle_service::ShuttleAxum {
    let router = Router::new()
        .route("/", get(root))
        .route("/ws", get(handler));
    let sync_wrapper = SyncWrapper::new(router);

    Ok(sync_wrapper)
}

async fn root() -> &'static str {
    "Hello, World!"
}

async fn handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    // first msg for socks5 request
    let buf = match socket.recv().await {
        Some(res) => match res {
            Ok(msg) => match msg {
                Message::Binary(data) => data,
                _ => return,
            },
            _ => return,
        },
        None => return,
    };

    // valid socks5 version and data length
    if 1 + 1 + (buf[1] as usize) != buf.len() || buf[0] != b'\x05' {
        return;
    }

    // send first resp with no auth
    if socket
        .send(Message::Binary(b"\x05\x00".to_vec()))
        .await
        .is_err()
    {
        return;
    }

    // second msg from socks with target address
    let buf = match socket.recv().await {
        Some(res) => match res {
            Ok(msg) => match msg {
                Message::Binary(data) => data,
                _ => return,
            },
            _ => return,
        },
        None => return,
    };

    // valid msg
    if buf.len() < 4 {
        return;
    }

    // parse socks command
    let ver = buf[0];
    let cmd = buf[1];
    let atyp = buf[3];

    // valid socks version
    if ver != b'\x05' {
        return;
    }

    // only support connect command
    if cmd != 1 {
        let _ = socket
            .send(Message::Binary(
                b"\x05\x07\x00\x01\x00\x00\x00\x00\x00\x00".to_vec(),
            ))
            .await;
        return;
    }

    // parse target address
    let addr;
    match atyp {
        1 => {
            // ipv4
            if buf.len() != 10 {
                return;
            }
            let dst_addr = IpAddr::V4(Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]));
            let dst_port = BigEndian::read_u16(&buf[8..]);
            addr = SocketAddr::new(dst_addr, dst_port).to_string();
        }
        3 => {
            // domain
            let offset = 4 + 1 + (buf[4] as usize);
            if offset + 2 != buf.len() {
                return;
            }
            let dst_port = BigEndian::read_u16(&buf[offset..]);
            let mut dst_addr = std::str::from_utf8(&buf[5..offset]).unwrap().to_string();
            dst_addr.push_str(":");
            dst_addr.push_str(&dst_port.to_string());
            addr = dst_addr;
        }
        4 => {
            // ipv6
            if buf.len() != 22 {
                return;
            }
            let dst_addr = IpAddr::V6(Ipv6Addr::new(
                ((buf[4] as u16) << 8) | buf[5] as u16,
                ((buf[6] as u16) << 8) | buf[7] as u16,
                ((buf[8] as u16) << 8) | buf[9] as u16,
                ((buf[10] as u16) << 8) | buf[11] as u16,
                ((buf[12] as u16) << 8) | buf[13] as u16,
                ((buf[14] as u16) << 8) | buf[15] as u16,
                ((buf[16] as u16) << 8) | buf[17] as u16,
                ((buf[18] as u16) << 8) | buf[19] as u16,
            ));
            let dst_port = BigEndian::read_u16(&buf[20..]);
            addr = SocketAddr::new(dst_addr, dst_port).to_string();
        }
        _ => {
            let _ = socket
                .send(Message::Binary(
                    b"\x05\x08\x00\x01\x00\x00\x00\x00\x00\x00".to_vec(),
                ))
                .await;
            return;
        }
    }

    // connect to target
    let mut outbound = match TcpStream::connect(addr).await {
        Ok(s) => s,
        _ => {
            let _ = socket
                .send(Message::Binary(
                    b"\x05\x01\x00\x01\x00\x00\x00\x00\x00\x00".to_vec(),
                ))
                .await;
            return;
        }
    };

    // send sencode resp ok
    if socket
        .send(Message::Binary(
            b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00".to_vec(),
        ))
        .await
        .is_err()
    {
        return;
    }

    // copy
    let mut inbound = WebSocketConnection(socket);
    let _ = copy_bidirectional(&mut inbound, &mut outbound).await;
}

#[pin_project]
pub struct WebSocketConnection(#[pin] WebSocket);

impl AsyncRead for WebSocketConnection {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project()
            .0
            .as_mut()
            .poll_next(cx)
            .map(|item| match item {
                Some(item) => match item {
                    Ok(msg) => match msg {
                        Message::Binary(data) => {
                            buf.put_slice(&data);
                            Ok(())
                        }
                        _ => Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "message type not support",
                        )),
                    },
                    Err(e) => Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!(
                            "get data from websocket connection error, detail is {:?}",
                            e
                        ),
                    )),
                },
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "websocket stream poll read fails".to_string(),
                )),
            })
    }
}

impl AsyncWrite for WebSocketConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let mut this = self.project();
        this.0.as_mut().poll_ready(cx).map(|item| match item {
            Ok(_) => match this.0.as_mut().start_send(Message::Binary(buf.to_vec())) {
                Ok(_) => Ok(buf.len()),
                Err(e) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("websocket stream start send fails, detail error is {:?}", e),
                )),
            },
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("websocket stream poll ready fails, detail error is {:?}", e),
            )),
        })
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.project()
            .0
            .as_mut()
            .poll_flush(cx)
            .map(|item| match item {
                Ok(_) => Ok(()),
                Err(e) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("websocket stream poll flush fails, detail error is {:?}", e),
                )),
            })
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.project()
            .0
            .as_mut()
            .poll_close(cx)
            .map(|item| match item {
                Ok(_) => Ok(()),
                Err(e) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("websocket stream close fails, detail error is {:?}", e),
                )),
            })
    }
}
