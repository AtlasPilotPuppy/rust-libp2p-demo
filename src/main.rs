#![feature(trivial_bounds)]

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
use log::{error, info};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::{fs, io::AsyncBufReadExt, sync::mpsc};

const STORAGE_FILE_PATH: &str = "./Memo.json";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

static KEYS: Lazy<identity::Keypair> = Lazy::new(|| identity::Keypair::generate_ed25519());
static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));
static TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("memo"));

type Memos = Vec<Memo>;

#[derive(Debug, Serialize, Deserialize)]
struct Memo {
    id: usize,
    title: String,
    body: String,
    public: bool,
}

#[derive(Debug, Serialize, Deserialize)]
enum ListMode {
    ALL,
    One(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct ListRequest {
    mode: ListMode,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListResponse {
    mode: ListMode,
    data: Memos,
    receiver: String,
}

#[derive(NetworkBehaviour)]
#[behaviour(event_process = true)]
struct MemoBehaviour {
    floodsub: Floodsub,
    mdns: Mdns,
    #[behaviour(ignore)]
    response_sender: mpsc::UnboundedSender<ListResponse>,
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for MemoBehaviour {
    fn inject_event(&mut self, event: FloodsubEvent) {
        match event {
            FloodsubEvent::Message(msg) => {
                if let Ok(resp) = serde_json::from_slice::<ListResponse>(&msg.data){
                    if resp.receiver == PEER_ID.to_string() {
                        info!("Response from sender: {}", msg.source);
                        resp.data.iter().for_each(|r| info!("{:?}", r));
                    }
                } else if let Ok(req) = serde_json::from_slice::<ListRequest>(&msg.data){
                    match req.mode {
                        ListMode::ALL => {
                            info!("Got ALL request: {:?} from {:?}", req, msg.source);
                            respond_with_public_memos(
                                self.response_sender.clone(),
                                msg.source.to_string(),
                            );
                        }
                        ListMode::One(ref peer_id) => {
                            if peer_id == &PEER_ID.to_string(){
                                info!("Received request: {:?} from {:?}", req, msg.source);
                                respond_with_public_memos(
                                    self.response_sender.clone(),
                                    msg.source.to_string(),
                                );
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    }
}

fn respond_with_public_memos(sender: mpsc::UnboundedSender<ListResponse>, receiver: String) {
    tokio::spawn(async move {
        match read_local_memos().await {
            Ok(memos) => {
                let resp = ListResponse {
                    mode: ListMode::ALL,
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

async fn read_local_memos() -> Result<Memos> {
    let content = fs::read(STORAGE_FILE_PATH).await?;
    let result = serde_json::from_slice(&content)?;
    Ok(result)
}

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

async fn publish_memo(id: usize) -> Result<()> {
    let mut local_memos = read_local_memos().await?;
    local_memos
        .iter_mut()
        .filter(|r| r.id == id)
        .for_each(|r| r.public = true);
    write_local_memos(&local_memos).await?;
    Ok(())
}

async fn write_local_memos(memos: &Memos) -> Result<()> {
    let json = serde_json::to_string(&memos)?;
    fs::write(STORAGE_FILE_PATH, &json).await?;
    Ok(())
}

async fn handle_list_peers(swarm: &mut Swarm<MemoBehaviour>) {
    info!("Found new Peers: ");
    let nodes = swarm.behaviour().mdns.discovered_nodes();
    let mut unique_peers = HashSet::new();
    for peer in nodes {
        unique_peers.insert(peer);
    }
    unique_peers.iter().for_each(|n| info!("node: {}", n));
}

async fn handle_list_memos(cmd: &str, swarm: &mut Swarm<MemoBehaviour>) {
    let rest = cmd.strip_prefix("ls m ");
    match rest {
        Some("all") => {
            let request = ListRequest{
                mode: ListMode::ALL,
            };
            let json = serde_json::to_string(&request).expect("Cant Jsonify Request.");
            swarm
                .behaviour_mut()
                .floodsub
                .publish(TOPIC.clone(), json.as_bytes());
        }
        Some(peer_id) => {
            let request = ListRequest {
                mode: ListMode::One(peer_id.to_owned()),
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

async fn handle_publish_recipe(cmd: &str) {
    if let Some(rest) = cmd.strip_prefix("publish m") {
        match rest.trim().parse::<usize>() {
            Ok(title) => {
                if let Err(e) = publish_memo(title).await {
                    info!("Error publishing memo title: {}, {}", title, e)
                } else {
                    info!("Published memo")
                }
            }
            Err(e) => error!("Title: {} is invalid. {}", rest.trim(), e),
        };
    }
}

async fn handle_create_memo(cmd: &str) {
    if let Some(rest) = cmd.strip_prefix("create m"){
        let elements: Vec<&str> = rest.split("|").map(|r| r.trim()).collect();
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
