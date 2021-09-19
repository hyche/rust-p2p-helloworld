#[macro_use]
extern crate lazy_static;

use futures::StreamExt;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::env;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jsonrpc_http_server::jsonrpc_core::{Error, IoHandler, Params};
use jsonrpc_http_server::{Server, ServerBuilder};
use libp2p::core::upgrade;
use libp2p::gossipsub::{
    Gossipsub, GossipsubConfigBuilder, GossipsubEvent, GossipsubMessage, IdentTopic,
    MessageAuthenticity, MessageId, ValidationMode,
};
use libp2p::mdns::{Mdns, MdnsEvent};
use libp2p::swarm::{SwarmBuilder, SwarmEvent};
use libp2p::{
    identity, mplex, noise, tcp::TokioTcpConfig, NetworkBehaviour, PeerId, Swarm, Transport,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

type Db = Arc<Mutex<HashMap<String, Pokemon>>>;

lazy_static! {
    static ref META_TOPIC: IdentTopic = IdentTopic::new("meta");
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Pokemon {
    name: String,
    color: String,

    #[serde(default)]
    eye_num: u32,
    #[serde(default)]
    nose_num: u32,
    #[serde(default)]
    mouth_num: u32,
}

#[derive(Deserialize)]
struct SearchParams {
    name: String,
}

#[derive(Debug)]
enum PeerEvent {
    Gossipsub(GossipsubEvent),
    Mdns(MdnsEvent),
}

impl From<MdnsEvent> for PeerEvent {
    fn from(v: MdnsEvent) -> Self {
        Self::Mdns(v)
    }
}
impl From<GossipsubEvent> for PeerEvent {
    fn from(v: GossipsubEvent) -> Self {
        Self::Gossipsub(v)
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "PeerEvent", event_process = false)]
struct PeerBehaviour {
    gossip: Gossipsub,
    mdns: Mdns,
}

async fn construct_network_swarm() -> Swarm<PeerBehaviour> {
    let id_keys = identity::Keypair::generate_ed25519();
    let peer_id = PeerId::from(id_keys.public());
    log::info!("Local peer id: {:?}", peer_id);

    let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
        .into_authentic(&id_keys)
        .expect("Signing libp2p-noise stati DH keypair failed.");

    // Set up an encrypted TCP Transport over the Mplex and Yamux protocols
    let transport = TokioTcpConfig::new()
        .nodelay(true)
        .upgrade(upgrade::Version::V1)
        .authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
        .multiplex(mplex::MplexConfig::new())
        .boxed();

    let mut swarm = {
        let message_id_fn = |message: &GossipsubMessage| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            MessageId::from(s.finish().to_string())
        };

        let gossipsub_config = GossipsubConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(ValidationMode::Strict)
            .message_id_fn(message_id_fn)
            .build()
            .expect("Valid config");

        let mut gossipsub: Gossipsub =
            Gossipsub::new(MessageAuthenticity::Signed(id_keys), gossipsub_config)
                .expect("Correct configuration");

        gossipsub.subscribe(&META_TOPIC).unwrap();

        let behaviour = PeerBehaviour {
            gossip: gossipsub,
            mdns: Mdns::new(Default::default()).await.unwrap(),
        };

        SwarmBuilder::new(transport, behaviour, peer_id)
            // We want the connection background tasks to be spawned
            // onto the tokio runtime.
            .executor(Box::new(|fut| {
                tokio::spawn(fut);
            }))
            .build()
    };

    swarm
        .listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap())
        .unwrap();

    swarm
}

fn construct_jsonrpc_server(
    mem_db: Db,
    sender: mpsc::Sender<String>,
    server_address: SocketAddr,
    server_threads: usize,
) -> Server {
    let _sender_create = sender.clone();
    let _sender_update = sender.clone();
    let _mem_db_create = Arc::clone(&mem_db);
    let _mem_db_update = Arc::clone(&mem_db);

    let mut io = IoHandler::default();
    io.add_method("get_pokemon", move |params: Params| {
        let db = Arc::clone(&mem_db);
        let sender = sender.clone();
        async move {
            let params: SearchParams = params.parse()?;
            sender
                .send(format!("Retrieve Pokemon named `{}`", &params.name))
                .await
                .unwrap();
            db.lock()
                .unwrap()
                .get(&params.name)
                .map(|value| serde_json::to_value(value).unwrap())
                .ok_or_else(|| Error::invalid_params("Entry not found!"))
        }
    });
    io.add_method("create_pokemon", move |params: Params| {
        let db = Arc::clone(&_mem_db_create);
        let sender = _sender_create.clone();
        async move {
            let pokemon: Pokemon = params.parse()?;
            sender
                .send(format!("Create Pokemon named `{}`", &pokemon.name))
                .await
                .unwrap();
            let db = &mut *db.lock().unwrap();
            if db.contains_key(&pokemon.name) {
                return Err(Error::invalid_params("Entry already exists!".to_string()));
            }
            let _ = db.insert(pokemon.name.clone(), pokemon.clone());
            Ok(serde_json::to_value(pokemon).unwrap())
        }
    });
    io.add_method("update_pokemon", move |params: Params| {
        let db = Arc::clone(&_mem_db_update);
        let sender = _sender_update.clone();
        async move {
            let pokemon: Pokemon = params.parse()?;
            sender
                .send(format!("Update Pokemon named `{}`", &pokemon.name))
                .await
                .unwrap();
            db.lock()
                .unwrap()
                .get_mut(&pokemon.name)
                .map(|value| {
                    *value = pokemon;
                    serde_json::to_value(value).unwrap()
                })
                .ok_or_else(|| Error::invalid_params("Entry not found!"))
        }
    });

    ServerBuilder::new(io)
        .threads(server_threads)
        .start_http(&server_address)
        .unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::init();

    let server_address: SocketAddr = env::var("SERVER_ADDRESS")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
        .parse()
        .expect("Unable to parse socket address.");

    let server_threads: usize = env::var("SERVER_THREADS")
        .unwrap_or_else(|_| "2".to_string())
        .parse()
        .expect("Invalid number of threads");

    let (tx, mut rx) = mpsc::channel::<String>(64);

    let mut swarm = construct_network_swarm().await;
    // Spawn tokio thread to handle swarm events
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                             log::info!("Listening on {:?}", address);
                        }
                        SwarmEvent::Behaviour(PeerEvent::Gossipsub(GossipsubEvent::Message {
                            propagation_source: peer_id,
                            message_id: id,
                            message,
                        })) => {
                             log::info!(
                                 "Got message: {} with id: {} from peer: {:?}",
                                 String::from_utf8_lossy(&message.data),
                                 id,
                                 peer_id
                             );
                        }
                        SwarmEvent::Behaviour(PeerEvent::Mdns(MdnsEvent::Discovered(list))) => {
                            for (peer, _) in list {
                                swarm.behaviour_mut().gossip.add_explicit_peer(&peer);
                            }
                        }
                        SwarmEvent::Behaviour(PeerEvent::Mdns(MdnsEvent::Expired(list))) => {
                            for (peer, _) in list {
                                if !swarm.behaviour_mut().mdns.has_node(&peer) {
                                    swarm.behaviour_mut().gossip.remove_explicit_peer(&peer);
                                }
                            }
                        }
                        _ => {}
                    }
                },
                message = rx.recv() => {
                    swarm
                        .behaviour_mut()
                        .gossip
                        .publish(META_TOPIC.clone(), message.unwrap()).unwrap();

                }
            }
        }
    });

    let mem_db = Arc::new(Mutex::new(HashMap::new()));
    let jsonrpc_server = construct_jsonrpc_server(mem_db, tx, server_address, server_threads);

    log::info!(
        "Start JSON-RPC server at {:?} with {:?} threads..",
        server_address,
        server_threads
    );

    jsonrpc_server.wait();

    Ok(())
}
