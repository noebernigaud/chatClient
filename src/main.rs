use std::{
    collections::HashMap,
    env,
    io::Error as IoError,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use chrono::prelude::Utc;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};

use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Duration};
use tungstenite::protocol::Message;

//use tokio_js_set_interval::{clear_interval, set_interval, set_timeout};

//use serde::{Deserialize, Serialize};

type Tx = UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<SocketAddr, Tx>>>;

// #[derive(Serialize, Deserialize)]
// struct MessageHash {
//     user: String,
//     time: String,
//     content: String,
// }

type SharedMessages = Arc<Mutex<Vec<Message>>>;

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    shared_messages: SharedMessages,
) {
    println!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .expect("Error during the websocket handshake occurred");
    println!("WebSocket connection established: {}", addr);

    // Insert the write part of this peer to the peer map.
    let (tx, rx) = unbounded();
    peer_map.lock().unwrap().insert(addr, tx);

    let (outgoing, incoming) = ws_stream.split();

    let broadcast_incoming = incoming.try_for_each(|msg| {
        println!(
            "Received a message from {}: {}",
            addr,
            msg.to_text().unwrap()
        );

        shared_messages.lock().unwrap().push(msg.clone());

        println!("messageStack updated: {:?}", shared_messages);

        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(broadcast_incoming, receive_from_others);
    future::select(broadcast_incoming, receive_from_others).await;

    println!("{} disconnected", &addr);
    peer_map.lock().unwrap().remove(&addr);
}

//Function in charge of broadcasting every second
async fn broadcast(peer_map: PeerMap, shared_messages: SharedMessages) {
    loop {
        sleep(Duration::from_millis(1000)).await;
        let peers = peer_map.lock().unwrap();
        let mut messages = shared_messages.lock().unwrap();

        let broadcast_recipients = peers
            .iter()
            //.filter(|(peer_addr, _)| peer_addr != &&addr)
            .map(|(_, ws_sink)| ws_sink);

        let message_list = messages.iter();

        for recp in broadcast_recipients {
            for msg in message_list.clone() {
                recp.unbounded_send(msg.clone()).unwrap();
            }
        }

        messages.clear();
        println!("finished broadcasting at {}", Utc::now());
    }
}

#[tokio::main]
async fn main() -> Result<(), IoError> {
    // let addr = env::args()
    //     .nth(1)
    //     .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{}", port);

    let state = PeerMap::new(Mutex::new(HashMap::new()));
    let message_stack = SharedMessages::new(Mutex::new(Vec::new()));

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {}", addr);

    //spawn a thread in charge of broadcasting
    tokio::spawn(broadcast(state.clone(), message_stack.clone()));

    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            state.clone(),
            stream,
            addr,
            message_stack.clone(),
        ));
    }

    Ok(())
}
