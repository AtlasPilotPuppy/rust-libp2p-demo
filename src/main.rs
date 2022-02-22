//! This module demonstrates using Rust with libp2p and tokio
//! The module can be ran in development mode using
//! create an empty json file `Memo.json` in the top level directory.
//! This file will be used to store and retrieve memos.
//! ```
//! RUST_LOG=info cargo run
//! ```
//! The module needs to be run or two or mode nodes on the same network.
//! The module can also be run on the same machine in two windows using `screen` or `tmux`
//! This module supports the following commands:
//! - `ls p` - List Peers.
//! - `ls m` - List local memos.
//! - `ls m all` - List all public memos from known peers.
//! - `ls m {peer_id}` - List all public memos from peer specified by peer_id.
//! - `create m Title|Body` - Create a new memo.
//! - `publish m {id}` - Set the public flag on the memo and publish to all known peers for storage.
//!

use libp2p::{
    core::upgrade,
    floodsub::{Floodsub, FloodsubEvent, Topic},
    futures::StreamExt,
    identity,
    mdns::{Mdns, MdnsEvent},
    mplex,
    NetworkBehaviour,
    PeerId,
    noise::{Keypair, NoiseConfig, X25519Spec},
    swarm::{NetworkBehaviourEventProcess, Swarm, SwarmBuilder},
    Transport,
};
use libp2p::tcp::TokioTcpConfig;
use log::{error, info};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::{fs, io::AsyncBufReadExt, sync::mpsc};

const STORAGE_FILE_PATH: &str = "./Memo.json";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

static KEYS: Lazy<identity::Keypair> = Lazy::new(identity::Keypair::generate_ed25519);
static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));
static TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("memo"));

type Memos = Vec<Memo>;

/// The memo type is the foundation of what is stored.
/// Memos are persisted in a json file
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Memo {
    id: usize,
    title: String,
    body: String,
    public: bool,
}

/// MemoMode is an enum that is used for matching the types of messages passed.
/// MemoMode is used to determine how the message is processed.
#[derive(Debug, Serialize, Deserialize)]
enum MemoMode {
    ALL,
    One(String),
    Publish,
    PublishResponse,
}

/// Struct to denote a memo request. It contains a reference to the [MemoMode] enum.
/// and an optional Memo object.
#[derive(Debug, Serialize, Deserialize)]
struct MemoRequest {
    mode: MemoMode,
    memo: Option<Memo>,
}


/// The MemoResponse Object contains the mode of the response and the data key contains a [Vec] of [Memos].
#[derive(Debug, Serialize, Deserialize)]
struct MemoResponse {
    mode: MemoMode,
    data: Memos,
    receiver: String,
}

/// [EventType] is an enum to handle responses from peers It contains:
/// either the [Response(MemoResponse)] - for denoting a response from peers
/// or the [Input(String)] - denoting an input from stdin.
enum EventType {
    Response(MemoResponse),
    Input(String),
}

/// [MemoBehaviour] - using the derive macro to derive implementatino from [libp2p::NetworkBehaviour].
/// This struct is used to handle the network behaviour between peers.
#[derive(NetworkBehaviour)]
#[behaviour(event_process = true)]
struct MemoBehaviour {
    floodsub: Floodsub,
    mdns: Mdns,
    #[behaviour(ignore)]
    response_sender: mpsc::UnboundedSender<MemoResponse>,
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for MemoBehaviour {
    /// implementation of inject_event from the trait
    fn inject_event(&mut self, event: FloodsubEvent) {
        match event {
            FloodsubEvent::Message(msg) => {
                if let Ok(resp) = serde_json::from_slice::<MemoResponse>(&msg.data){
                    /// Parse an Ok [MemoResponse]
                    if resp.receiver == PEER_ID.to_string() {
                        info!("Response from sender: {}", msg.source);
                        resp.data.iter().for_each(|r| info!("{:?}", r));
                    }
                } else if let Ok(req) = serde_json::from_slice::<MemoRequest>(&msg.data){
                    match req.mode {
                        MemoMode::ALL => {
                            /// parse [MemoMode::ALL] to display all public memos of peers.
                            info!("Got ALL request: {:?} from {:?}", req, msg.source);
                            respond_with_public_memos(
                                self.response_sender.clone(),
                                msg.source.to_string(),
                            );
                        }
                        MemoMode::One(ref peer_id) => {
                            /// Handle specific peer response
                            if peer_id == &PEER_ID.to_string(){
                                info!("Received One request: {:?} from {:?}", req, msg.source);
                                respond_with_public_memos(
                                    self.response_sender.clone(),
                                    msg.source.to_string(),
                                );
                            }
                        }
                        /// Handle the publish request [MemoMode::Publish].
                        MemoMode::Publish => {
                            info!("Received Publish request: {:?} from {:?}", req, msg.source);
                            let memo = req.memo.unwrap();
                            add_published_memo(
                                memo,
                                self.response_sender.clone(),
                                msg.source.to_string(),
                            );
                        }
                        /// Handle the [MemoMode::PublishResponse]
                        MemoMode::PublishResponse => {
                            info!("Got a publish response: {:?} - {:?}", req, msg.source);

                        }
                    }
                }
            }
            _ => (),
        }
    }
}
/// function to add a published memo.
fn add_published_memo(memo: Memo, _sender: mpsc::UnboundedSender<MemoResponse>, receiver: String) {
    tokio::spawn(async move {
        match read_local_memos().await {
            Ok(mut memos) => {
                memos.push(memo);
                write_local_memos(&memos).await;
                let _resp = MemoResponse {
                    mode: MemoMode::PublishResponse,
                    receiver,
                    data: memos.into_iter().filter(|m| m.public).collect(),
                };
            }
            Err(e) => error!("error fetching local memos for ALL req {}", e),
        }
    });
}

/// Function to respond with public memos.
fn respond_with_public_memos(sender: mpsc::UnboundedSender<MemoResponse>, receiver: String) {
    tokio::spawn(async move {
        match read_local_memos().await {
            Ok(memos) => {
                let resp = MemoResponse {
                    mode: MemoMode::ALL,
                    receiver,
                    data: memos.into_iter().filter(|m| m.public).collect(),
                };
                if let Err(e) = sender.send(resp) {
                    error!("error sending response via : {}", e);
                }
            }
            Err(e) => error!("error fetching local memos for ALL req {}", e),
        }
    });
}

/// async function to read local memos
async fn read_local_memos() -> Result<Memos> {
    let content = fs::read(STORAGE_FILE_PATH).await?;
    let result = serde_json::from_slice(&content)?;
    Ok(result)
}

/// Handle MdnsEvents locate new peers.
impl NetworkBehaviourEventProcess<MdnsEvent> for MemoBehaviour {
    fn inject_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(discovered_list) => {
                for (peer, _addr) in discovered_list {
                    self.floodsub.add_node_to_partial_view(peer);
                }
            }
            MdnsEvent::Expired(expired_list) => {
                for (peer, _addr) in expired_list {
                    if !self.mdns.has_node(&peer) {
                        self.floodsub.remove_node_from_partial_view(&peer);
                    }
                }
            }
        }
    }
}

/// Create a new [Memo] entry in [Memos] and persist it to the local json.
async fn create_new_memo(title: &str, body: &str) -> Result<()> {
    let mut local_memos = read_local_memos().await?;
    let new_id = match local_memos.iter().max_by_key(|r| r.id) {
        Some(v) => v.id + 1,
        None => 0,
    };

    local_memos.push(Memo {
        id: new_id,
        title: title.to_owned(),
        body: body.to_owned(),
        public: false,
    });
    write_local_memos(&local_memos).await?;
    info!("Added memo- Title: {}, body: {}", title, body);
    Ok(())
}

/// async function to publish a local [Memo] after setting its public flag.
async fn publish_memo(id: usize) -> Result<Memo> {
    let mut local_memos = read_local_memos().await?;
    local_memos
        .iter_mut()
        .filter(|r| r.id == id)
        .for_each(|r| r.public = true);
    write_local_memos(&local_memos).await?;
    let memo_option = &local_memos.iter().find(|r| r.id == id).unwrap();
    Ok((*memo_option).clone())
}

/// Persist [Memos] to local json.
async fn write_local_memos(memos: &Memos) -> Result<()> {
    let json = serde_json::to_string(&memos)?;
    fs::write(STORAGE_FILE_PATH, &json).await?;
    Ok(())
}

/// Log list of unique peers.
async fn handle_list_peers(swarm: &mut Swarm<MemoBehaviour>) {
    info!("Found new Peers: ");
    let nodes = swarm.behaviour().mdns.discovered_nodes();
    let mut unique_peers = HashSet::new();
    for peer in nodes {
        unique_peers.insert(peer);
    }
    unique_peers.iter().for_each(|n| info!("node: {}", n));
}

/// Handle the peer response for [Memos].
async fn handle_list_memos(cmd: &str, swarm: &mut Swarm<MemoBehaviour>) {
    let rest = cmd.strip_prefix("ls m ");
    match rest {
        Some("all") => {
            let request = MemoRequest{
                mode: MemoMode::ALL,
                memo: None,
            };
            let json = serde_json::to_string(&request).expect("Cant Jsonify Request.");
            swarm
                .behaviour_mut()
                .floodsub
                .publish(TOPIC.clone(), json.as_bytes());
        }
        Some(peer_id) => {
            let request = MemoRequest {
                mode: MemoMode::One(peer_id.to_owned()),
                memo: None,
            };
            let json = serde_json::to_string(&request).expect("Cant Jsonify Request.");
            swarm
                .behaviour_mut()
                .floodsub
                .publish(TOPIC.clone(), json.as_bytes());
        }
        None => {
            match read_local_memos().await {
                Ok(memos) => {
                    info!("Local Memo ({})", memos.len());
                    memos.iter().for_each(|m| info!("{:?}", m));
                }
                Err(e) => error!("Error fetching memos: {}", e),
            };
        }

    };
}

/// Handle the request to publish [Memo]
async fn handle_publish_memos(cmd: &str, swarm: &mut Swarm<MemoBehaviour>) {
    if let Some(rest) = cmd.strip_prefix("publish m") {
        match rest.trim().parse::<usize>() {
            Ok(id) => {
                match  publish_memo(id).await {
                    Err(e)=>{
                        info!("Error publishing memo title: {}, {}", id, e)
                    }
                    Ok(memo) => {
                        info!("Published memo");
                        let request = MemoRequest {
                            mode: MemoMode::Publish,
                            memo: Some(memo),
                        };
                        let json = serde_json::to_string(&request).expect("Cant Jsonify Request.");
                        swarm
                            .behaviour_mut()
                            .floodsub
                            .publish(TOPIC.clone(), json.as_bytes());
                        info!("PUBLISHED JSON");
                    }
                }
            }
            Err(e) => error!("Title: {} is invalid. {}", rest.trim(), e),
        };
    }
}

/// Handle the request to create a new [Memo] and add persist it locally.
async fn handle_create_memo(cmd: &str) {
    if let Some(rest) = cmd.strip_prefix("create m"){
        let elements: Vec<&str> = rest.split('|').map(|r| r.trim()).collect();
        if elements.len() < 2 {
            error!("Too few arguments: Format: Title| Body");
        } else {
            let title = elements.get(0).expect("Name not found");
            let body = elements.get(1).expect("Body not found");
            if let Err(e) = create_new_memo(title, body).await {
                error!("Error creating memo: {}", e);
            };
        }
    }
}

/// Main Tokio event loop.
/// This handles the creation of the libp2p client, connecting to peers and handling
/// Requests and responses.

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    info!("PeerID: {}", PEER_ID.clone());
    let (response_sender, mut response_rcv) = mpsc::unbounded_channel();

    let auth_keys = Keypair::<X25519Spec>::new()
        .into_authentic(&KEYS)
        .expect("Can't create auth keys.");

    let transport = TokioTcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(NoiseConfig::xx(auth_keys).into_authenticated())
        .multiplex(mplex::MplexConfig::new())
        .boxed();

    let mut behaviour = MemoBehaviour {
        floodsub: Floodsub::new(*PEER_ID),
        mdns: Mdns::new(Default::default())
            .await
            .expect("Cant create mdns"),
        response_sender,
    };

    behaviour.floodsub.subscribe(TOPIC.clone());

    let mut swarm = SwarmBuilder::new(transport, behaviour, *PEER_ID)
        .executor(Box::new(|future|{tokio::spawn(future);}))
        .build();
    /// Take values from stdin line by line.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    /// Start the p2p swarm.
    Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .expect("Can't get socket")
    )
        .expect("Can't start swarm.");

    loop{
        let evt = {
            /// handle responses and events
            tokio::select! {
                line = stdin.next_line() => Some(EventType::Input(line.expect("Cant get line").expect("Cant read line"))),
                response = response_rcv.recv() => Some(EventType::Response(response.expect("Response error"))),
                event = swarm.select_next_some() => {
                    info!("Unhandled swarm event: {:?}", event);
                    None
                },
            }
        };

        if let Some(event) = evt {
            match event {
                EventType::Response(resp) => {
                    let json = serde_json::to_string(&resp).expect("cant Jsonify");
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(TOPIC.clone(), json.as_bytes());
                }
                /// Handle the inputs from stdin.
                EventType::Input(line) => match line.as_str() {
                    "ls p" => handle_list_peers(&mut swarm).await,
                    cmd if cmd.starts_with("ls m") => handle_list_memos(cmd, &mut swarm).await,
                    cmd if cmd.starts_with("create m") => handle_create_memo(cmd).await,
                    cmd if cmd.starts_with("publish m") => handle_publish_memos(cmd, &mut swarm).await,
                    _ => error!("Invalid command"),
                },
            }
        }
    }
}
