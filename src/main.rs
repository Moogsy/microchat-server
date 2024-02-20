use std::{collections::HashMap, error::Error, net::SocketAddr, sync::{Arc, Mutex}};

use futures_channel::mpsc::UnboundedSender;
use futures_util::{future, pin_mut, StreamExt, TryStreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

static ADDR: &str = "127.0.0.1:8080";

type MessageSender = UnboundedSender<Message>;
type PeerMap = HashMap<SocketAddr, MessageSender>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {

    let state = Arc::new(Mutex::new(PeerMap::new()));
    let listener = TcpListener::bind(ADDR).await?;

    while let Ok((stream, address)) = listener.accept().await {
        let cloned = Arc::clone(&state);
        let future = accept_connection(cloned, stream, address);
        tokio::spawn(future);
    }

    Ok(())
}

async fn accept_connection(
    state: Arc<Mutex<PeerMap>>, 
    raw_stream: TcpStream, 
    address: SocketAddr
) {
    log::info!("Incoming TCP connection from: {address}");
    if let Err(err) = handle_connection(state, raw_stream, address).await {
        log::warn!("Error handling connection: {err:?}");
    }
    log::info!("Closed TCP connection with {address}");
}

async fn handle_connection(
    state: Arc<Mutex<PeerMap>>, 
    raw_stream: TcpStream, 
    address: SocketAddr
) -> Result<(), Box<dyn Error>> {
    let ws_stream = tokio_tungstenite::accept_async(raw_stream).await?;
    log::info!("Websocket connection established with {address}");

    let (tx, rx) = futures_channel::mpsc::unbounded();
    state
        .lock()
        // The unwrap will only fail if another thread
        // panicked while holding this lock
        // so we are better off propagating the error
        .unwrap()
        .insert(address, tx);

    let (outgoing, incoming) = ws_stream.split();

    let broadcast_incoming = incoming.try_for_each_concurrent(None, |msg| {
        log::info!("Received a message from {address}: {}", msg.to_string());

        let peers = state.lock().unwrap();
        let broadcast_recipients = peers
            .iter()
            .filter(|(peer_addr, _)| **peer_addr != address)
            .map(|(_, ws_sink)| ws_sink);

        for sender in broadcast_recipients {
            if let Err(err) = sender.unbounded_send(msg.clone()) {
                eprintln!("Failed to broadcast message from {address}: {err}");
            };
        }

        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);
    pin_mut!(broadcast_incoming, receive_from_others);

    future::select(broadcast_incoming, receive_from_others).await;

    println!("{address} disconnected");
    state.lock().unwrap().remove(&address);

    Ok(())
}