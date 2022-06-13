use std::{
    collections::HashMap,
    env,
    io::Error as IoError,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use chrono::{prelude::Utc, Timelike};

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};

use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Duration};
use tungstenite::protocol::Message;

//use tokio_js_set_interval::{clear_interval, set_interval, set_timeout};

//use serde::{Deserialize, Serialize};
use serde_json;

use rand::seq::SliceRandom;

type Tx = UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<SocketAddr, Tx>>>;

// #[derive(Serialize, Deserialize)]
// struct MessageHash {
//     user: String,
//     time: String,
//     content: String,
// }

type SharedMessages = Arc<Mutex<Vec<Message>>>;
type UserList = Arc<Mutex<Vec<Message>>>;

async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    shared_messages: SharedMessages,
    user_list: UserList,
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

    let mut username = String::new();
    let colors = vec!["red", "blue", "orange", "green", "purple"];
    let color = *colors.choose(&mut rand::thread_rng()).unwrap();

    let broadcast_incoming = incoming.try_for_each(|msg| {
        //if the message is a keepalive, we do nothing
        if msg.to_text().unwrap() == "keepalive" {
            println!("Keep alive from {}", addr)
        }
        //print the message in server and add it to the message stack
        else {
            let msg_text = msg.to_text().unwrap();

            println!("Received a message from {}: {}", addr, msg_text);

            let json_res: Result<serde_json::Value, serde_json::Error> =
                serde_json::from_str(msg_text);

            match json_res {
                Ok(json) => {
                    match json["route"].as_str().expect("not a string") {
                        //login and message route
                        "message" => {
                            let color_field =
                                HashMap::from([(String::from("color"), String::from(color))]);
                            let return_json = merge(&json, &color_field);
                            let return_message = return_json.to_string();
                            shared_messages
                                .lock()
                                .unwrap()
                                .push(Message::Text(return_message));
                            println!("messageStack updated: {:?}", shared_messages);
                        }
                        "login" => {
                            let json_content = json["content"]
                                .as_str()
                                .expect("username received is not a string");
                            username = json_content.to_string();
                            user_list
                                .lock()
                                .unwrap()
                                .push(Message::Text(username.clone()));
                            shared_messages.lock().unwrap().push(msg.clone());
                            println!("messageStack updated: {:?}", shared_messages);
                        }
                        //keep alive route, does nothing
                        "keepalive" => {}
                        //invalid route not matching any of the previous patterns
                        s => println!("the message's route attribute: {} is not valid.", s),
                    }
                }
                Err(_) => {
                    println!("message text {} can't be converted to JSON", msg_text);
                }
            }
        }
        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(broadcast_incoming, receive_from_others);
    future::select(broadcast_incoming, receive_from_others).await;

    println!("{} disconnected", &addr);

    //send a message in the chat to leet chaters know this user was disconnected
    let logout_message = format!(r#" {{"route": "logout", "content": "{}"}} "#, username);
    shared_messages
        .lock()
        .unwrap()
        .push(Message::Text(logout_message.to_string()));

    //remove the user from the current user list
    let mut locked_user_list = user_list.lock().unwrap();
    if let Some(index) = locked_user_list
        .iter()
        .position(|value| *value.to_string() == username)
    {
        locked_user_list.swap_remove(index);
    }

    //remove the user from the lsit of connections
    peer_map.lock().unwrap().remove(&addr);
}

//Function in charge of broadcasting every second
async fn broadcast(peer_map: PeerMap, shared_messages: SharedMessages, user_list: UserList) {
    loop {
        sleep(Duration::from_millis(1000)).await;
        let peers = peer_map.lock().unwrap();
        let mut messages = shared_messages.lock().unwrap();
        let users = user_list.lock().unwrap();

        let broadcast_recipients = peers
            .iter()
            //.filter(|(peer_addr, _)| peer_addr != &&addr)
            .map(|(_, ws_sink)| ws_sink);

        let message_list = messages.iter();
        let user_iterator = users.iter();

        let mut user_number = 0;

        for recp in broadcast_recipients {
            user_number += 1;
            for msg in message_list.clone() {
                recp.unbounded_send(msg.clone()).unwrap();
            }
            let mut user_list_message = String::from(r#" {"route":"userlist", "content":" "#);
            for user in user_iterator.clone() {
                user_list_message.push_str(user.to_text().unwrap());
                user_list_message.push_str(", ");
            }
            user_list_message.pop();
            user_list_message.pop();
            user_list_message.push_str("\"}");
            recp.unbounded_send(Message::from(user_list_message))
                .unwrap();
        }

        messages.clear();
        println!(
            "finished broadcasting ({}s) to {} users",
            Utc::now().second(),
            user_number
        );
    }
}

pub fn merge(v: &serde_json::Value, fields: &HashMap<String, String>) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut m = m.clone();
            for (k, v) in fields {
                m.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            serde_json::Value::Object(m)
        }
        v => v.clone(),
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
    let users = UserList::new(Mutex::new(Vec::new()));

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {}", addr);

    //spawn a thread in charge of broadcasting
    tokio::spawn(broadcast(
        state.clone(),
        message_stack.clone(),
        users.clone(),
    ));

    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            state.clone(),
            stream,
            addr,
            message_stack.clone(),
            users.clone(),
        ));
    }

    Ok(())
}
