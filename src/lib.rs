#![feature(trivial_bounds)]

use libp2p::{
    floodsub::{Floodsub, FloodsubEvent, Topic},
    futures::StreamExt,
    identity,
    mdns::{Mdns, MdnsEvent},
    NetworkBehaviour,
    PeerId,
    swarm::{NetworkBehaviourEventProcess},
    // tcp::TokioTcpConfig,
    // NetworkBehaviour, PeerId, Transport,
};
use log::{error, info};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use tokio::{fs, sync::mpsc};

const STORAGE_FILE_PATH: &str = "./Memo.json";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

static KEYS: Lazy<identity::Keypair> = Lazy::new(identity::Keypair::generate_ed25519);
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
