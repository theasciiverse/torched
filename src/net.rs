//! Network play over iroh: the host and its peers run the same game and
//! exchange only commands. The host puts every command in one order and
//! sends it to everyone, itself included; each peer applies them as its
//! game reaches them. The host also sends a fingerprint of its game after
//! each command: a peer that differs (floating point can differ between
//! machines) asks for the host's whole game and carries on from that.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::time::Duration;

use iroh::endpoint::{RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::cmd::Cmd;
use crate::sim::{Config, Game};

pub const ALPN: &[u8] = b"torched/1";
const TICKET_PREFIX: &str = "torched";
/// Biggest message accepted: a whole game is well under this.
const MAX_FRAME: usize = 8 << 20;

/// Who plays a tank in a network game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Owner {
    /// A person at this peer's keyboard (0 is the host).
    Peer(u8),
    Computer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Hello { version: String, name: String },
    Welcome { peer: u8 },
    Refused { why: String },
    /// Who has joined so far.
    Lobby { names: Vec<String> },
    /// The game: settings and seed, the data files, names and owners.
    Start { cfg: Config, weapons: String, accessories: String, names: Vec<String>, owners: Vec<Owner> },
    /// A command in the one true order.
    Cmd { seq: u64, tank: u8, cmd: Cmd },
    /// The host's fingerprint after applying `seq`.
    Check { seq: u64, sum: u64 },
    /// A peer's command, for the host to put in order.
    Want { tank: u8, cmd: Cmd },
    ResyncPlease,
    /// The host's whole game, as of `seq`.
    Snapshot { seq: u64, game: Vec<u8> },
}

/// From the network thread to the game.
#[derive(Debug)]
pub enum In {
    Ready { ticket: String },
    Joined { peer: u8, name: String },
    Left { peer: u8 },
    Got { from: u8, msg: Msg },
    Failed(String),
}

enum Out {
    To(u8, Msg),
    All(Msg),
}

/// The connection: iroh on a thread of its own.
pub struct Link {
    pub host: bool,
    out: mpsc::UnboundedSender<Out>,
    inbox: std_mpsc::Receiver<In>,
}

pub fn encode_ticket(addr: &EndpointAddr) -> String {
    let bytes = postcard::to_allocvec(addr).unwrap_or_default();
    format!("{TICKET_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
}

pub fn decode_ticket(t: &str) -> Result<EndpointAddr, String> {
    let body = t.trim().strip_prefix(TICKET_PREFIX).ok_or("that isn't a Torched ticket")?;
    let bytes = data_encoding::BASE32_NOPAD.decode(body.to_ascii_uppercase().as_bytes()).map_err(|_| "the ticket is garbled")?;
    postcard::from_bytes(&bytes).map_err(|_| "the ticket is garbled".to_string())
}

async fn write_msg(send: &mut SendStream, m: &Msg) -> Result<(), String> {
    let b = postcard::to_allocvec(m).map_err(|e| e.to_string())?;
    send.write_all(&(b.len() as u32).to_le_bytes()).await.map_err(|e| e.to_string())?;
    send.write_all(&b).await.map_err(|e| e.to_string())
}

async fn read_msg(recv: &mut RecvStream) -> Result<Msg, String> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.map_err(|e| e.to_string())?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err("message too big".into());
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
    postcard::from_bytes(&buf).map_err(|e| e.to_string())
}

/// Copy messages from a channel onto a stream until either ends.
async fn writer(mut send: SendStream, mut rx: mpsc::UnboundedReceiver<Msg>) {
    while let Some(m) = rx.recv().await {
        if let Err(e) = write_msg(&mut send, &m).await {
            warn!("send: {e}");
            break;
        }
    }
}

impl Link {
    /// Start hosting. `local` keeps to direct addresses (tests, LANs with
    /// no internet); otherwise iroh's relays and lookup are used too.
    pub fn host(local: bool) -> Link {
        let (out, mut out_rx) = mpsc::unbounded_channel::<Out>();
        let (in_tx, inbox) = std_mpsc::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return drop(in_tx.send(In::Failed(e.to_string()))),
            };
            rt.block_on(async move {
                let ep = if local {
                    Endpoint::builder(presets::Minimal).alpns(vec![ALPN.to_vec()]).bind().await
                } else {
                    Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]).bind().await
                };
                let ep = match ep {
                    Ok(ep) => ep,
                    Err(e) => return drop(in_tx.send(In::Failed(e.to_string()))),
                };
                if !local {
                    // A relay address makes the ticket work across the
                    // internet; don't wait forever for one.
                    let _ = tokio::time::timeout(Duration::from_secs(5), ep.online()).await;
                }
                let ticket = encode_ticket(&ep.addr());
                info!("hosting, ticket {ticket}");
                let _ = in_tx.send(In::Ready { ticket });
                let peers: Arc<Mutex<HashMap<u8, mpsc::UnboundedSender<Msg>>>> = Arc::default();
                // Messages from the game to the peers.
                let p2 = peers.clone();
                tokio::spawn(async move {
                    while let Some(o) = out_rx.recv().await {
                        let peers = p2.lock().expect("peers lock");
                        match o {
                            Out::To(p, m) => {
                                if let Some(tx) = peers.get(&p) {
                                    let _ = tx.send(m);
                                }
                            }
                            Out::All(m) => {
                                for tx in peers.values() {
                                    let _ = tx.send(m.clone());
                                }
                            }
                        }
                    }
                });
                let mut next_peer = 1u8;
                while let Some(incoming) = ep.accept().await {
                    let Ok(conn) = incoming.await else { continue };
                    let peer = next_peer;
                    next_peer = next_peer.saturating_add(1);
                    let (peers, in_tx) = (peers.clone(), in_tx.clone());
                    tokio::spawn(async move {
                        let Ok((mut send, mut recv)) = conn.accept_bi().await else { return };
                        let Ok(Msg::Hello { version, name }) = read_msg(&mut recv).await else { return };
                        if version != env!("CARGO_PKG_VERSION") {
                            let why = format!("the host runs Torched {}, you run {version}", env!("CARGO_PKG_VERSION"));
                            let _ = write_msg(&mut send, &Msg::Refused { why }).await;
                            let _ = send.finish();
                            conn.closed().await;
                            return;
                        }
                        let (tx, rx) = mpsc::unbounded_channel();
                        let _ = tx.send(Msg::Welcome { peer });
                        peers.lock().expect("peers lock").insert(peer, tx);
                        let _ = in_tx.send(In::Joined { peer, name });
                        tokio::spawn(writer(send, rx));
                        while let Ok(msg) = read_msg(&mut recv).await {
                            if in_tx.send(In::Got { from: peer, msg }).is_err() {
                                break;
                            }
                        }
                        peers.lock().expect("peers lock").remove(&peer);
                        let _ = in_tx.send(In::Left { peer });
                    });
                }
            });
        });
        Link { host: true, out, inbox }
    }

    /// Join a host by its ticket.
    pub fn join(ticket: &str, name: String, local: bool) -> Result<Link, String> {
        let addr = decode_ticket(ticket)?;
        let (out, mut out_rx) = mpsc::unbounded_channel::<Out>();
        let (in_tx, inbox) = std_mpsc::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return drop(in_tx.send(In::Failed(e.to_string()))),
            };
            rt.block_on(async move {
                let ep = if local { Endpoint::bind(presets::Minimal).await } else { Endpoint::bind(presets::N0).await };
                let ep = match ep {
                    Ok(ep) => ep,
                    Err(e) => return drop(in_tx.send(In::Failed(e.to_string()))),
                };
                let conn = match tokio::time::timeout(Duration::from_secs(20), ep.connect(addr, ALPN)).await {
                    Ok(Ok(c)) => c,
                    Ok(Err(e)) => return drop(in_tx.send(In::Failed(format!("couldn't reach the host: {e}")))),
                    Err(_) => return drop(in_tx.send(In::Failed("couldn't reach the host in time".into()))),
                };
                let (mut send, mut recv) = match conn.open_bi().await {
                    Ok(s) => s,
                    Err(e) => return drop(in_tx.send(In::Failed(e.to_string()))),
                };
                let hello = Msg::Hello { version: env!("CARGO_PKG_VERSION").into(), name };
                if let Err(e) = write_msg(&mut send, &hello).await {
                    return drop(in_tx.send(In::Failed(e)));
                }
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(writer(send, rx));
                tokio::spawn(async move {
                    while let Some(o) = out_rx.recv().await {
                        let (Out::To(_, m) | Out::All(m)) = o;
                        if tx.send(m).is_err() {
                            break;
                        }
                    }
                });
                while let Ok(msg) = read_msg(&mut recv).await {
                    if in_tx.send(In::Got { from: 0, msg }).is_err() {
                        break;
                    }
                }
                let _ = in_tx.send(In::Left { peer: 0 });
                ep.close().await;
            });
        });
        Ok(Link { host: false, out, inbox })
    }

    pub fn to(&self, peer: u8, m: Msg) {
        let _ = self.out.send(Out::To(peer, m));
    }

    pub fn all(&self, m: Msg) {
        let _ = self.out.send(Out::All(m));
    }

    pub fn poll(&self) -> Vec<In> {
        self.inbox.try_iter().collect()
    }
}

/// A network game in progress: the lockstep around a `Game`.
pub struct Session {
    pub link: Link,
    /// This peer's number: 0 for the host.
    pub me: u8,
    pub owners: Vec<Owner>,
    /// Commands in order, waiting until the game reaches them.
    pending: VecDeque<(u64, usize, Cmd)>,
    /// Host: the next number to give out. Peers: the last applied.
    next_seq: u64,
    last_applied: u64,
    host_sums: HashMap<u64, u64>,
    my_sums: HashMap<u64, u64>,
    resyncing: bool,
    /// Times this peer has taken the host's game to get back in step.
    pub resyncs: u32,
    pub status: String,
    /// Ticks since the last Aim was sent, to keep them sparse.
    aim_wait: u32,
    last_aim: Option<Cmd>,
}

impl Session {
    pub fn new(link: Link, me: u8, owners: Vec<Owner>) -> Session {
        Session {
            link,
            me,
            owners,
            pending: VecDeque::new(),
            next_seq: 1,
            last_applied: 0,
            host_sums: HashMap::new(),
            my_sums: HashMap::new(),
            resyncing: false,
            resyncs: 0,
            status: String::new(),
            aim_wait: 0,
            last_aim: None,
        }
    }

    /// Played at this keyboard.
    pub fn is_local(&self, tank: usize) -> bool {
        self.owners.get(tank) == Some(&Owner::Peer(self.me))
    }

    /// A command from this keyboard.
    pub fn submit(&mut self, tank: usize, cmd: Cmd) {
        if self.link.host {
            // A second Enter before the first Advance is applied would
            // send everyone on twice; drop it here, before it is numbered,
            // so every numbered command applies everywhere.
            if matches!(cmd, Cmd::Advance { .. }) && self.pending.iter().any(|(_, _, c)| matches!(c, Cmd::Advance { .. })) {
                return;
            }
            self.order(tank, cmd);
        } else {
            self.link.all(Msg::Want { tank: tank as u8, cmd });
        }
    }

    /// Host: number a command and send it to everyone.
    fn order(&mut self, tank: usize, cmd: Cmd) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.link.all(Msg::Cmd { seq, tank: tank as u8, cmd: cmd.clone() });
        self.pending.push_back((seq, tank, cmd));
    }

    /// Send the aim of a local player now and then, so others see it.
    pub fn share_aim(&mut self, game: &Game) {
        self.aim_wait += 1;
        if game.phase != crate::sim::Phase::Aim || !self.is_local(game.turn) || self.aim_wait < 8 {
            return;
        }
        self.aim_wait = 0;
        let aim = game.aim_of(game.turn, false);
        if self.last_aim.as_ref() != Some(&aim) {
            self.last_aim = Some(aim.clone());
            self.submit(game.turn, aim);
        }
    }

    /// Network news, then whatever commands now apply. Returns a reason
    /// if the game is over for this peer (the host left).
    pub fn pump(&mut self, game: &mut Game) -> Option<String> {
        for ev in self.link.poll() {
            match ev {
                In::Got { from, msg } => {
                    if let Some(end) = self.handle(game, from, msg) {
                        return Some(end);
                    }
                }
                In::Left { peer } => {
                    if !self.link.host {
                        return Some("The host left the game.".into());
                    }
                    // A computer takes over whoever left.
                    let theirs: Vec<usize> = (0..self.owners.len()).filter(|&t| self.owners[t] == Owner::Peer(peer)).collect();
                    for t in theirs {
                        self.owners[t] = Owner::Computer;
                        self.order(t, Cmd::Takeover);
                    }
                    self.status = format!("Player {peer} left; a computer plays on for them.");
                }
                In::Failed(e) => return Some(e),
                In::Joined { .. } | In::Ready { .. } => {}
            }
        }
        // Apply what the game has reached.
        // Every numbered command applies, in order, once the game reaches
        // it: a peer still watching the last shell land waits for the
        // host's Advance rather than dropping it.
        while let Some((seq, tank, cmd)) = self.pending.front().cloned() {
            if !game.can_apply(tank, &cmd) {
                break;
            }
            self.pending.pop_front();
            // A player's own Aim, coming back, would undo newer turns of
            // the dial; their screen is already right.
            let own_aim = matches!(cmd, Cmd::Aim { .. }) && self.is_local(tank);
            if !own_aim {
                game.apply(tank, &cmd);
            }
            self.last_applied = seq;
            let sum = game.checksum();
            if self.link.host {
                self.link.all(Msg::Check { seq, sum });
            } else {
                self.my_sums.insert(seq, sum);
                self.compare(seq);
            }
        }
        None
    }

    fn compare(&mut self, seq: u64) {
        if let (Some(h), Some(m)) = (self.host_sums.get(&seq), self.my_sums.get(&seq)) {
            if h != m && !self.resyncing {
                warn!("out of step at command {seq}; asking the host for its game");
                self.resyncing = true;
                self.link.all(Msg::ResyncPlease);
            }
            self.host_sums.remove(&seq);
            self.my_sums.remove(&seq);
        }
    }

    fn handle(&mut self, game: &mut Game, from: u8, msg: Msg) -> Option<String> {
        match msg {
            Msg::Cmd { seq, tank, cmd } if !self.link.host => {
                if seq > self.last_applied && self.pending.back().is_none_or(|(s, _, _)| *s < seq) {
                    self.pending.push_back((seq, tank as usize, cmd));
                }
            }
            Msg::Check { seq, sum } if !self.link.host => {
                if seq > self.last_applied || self.my_sums.contains_key(&seq) {
                    self.host_sums.insert(seq, sum);
                    self.compare(seq);
                }
            }
            Msg::Want { tank, cmd } if self.link.host => {
                // Only for a tank that peer plays.
                if self.owners.get(tank as usize) == Some(&Owner::Peer(from)) {
                    self.order(tank as usize, cmd);
                }
            }
            Msg::ResyncPlease if self.link.host => {
                if let Ok(bytes) = postcard::to_allocvec(&*game) {
                    // The game as of the last command applied here; later
                    // ones are already on their way.
                    let seq = self.last_applied;
                    self.link.to(from, Msg::Snapshot { seq, game: bytes });
                }
            }
            Msg::Snapshot { seq, game: bytes } if !self.link.host => match postcard::from_bytes::<Game>(&bytes) {
                Ok(mut g) => {
                    g.arsenal = game.arsenal.clone();
                    g.accessories = game.accessories.clone();
                    *game = g;
                    self.last_applied = seq;
                    self.pending.retain(|(s, _, _)| *s > seq);
                    self.host_sums.retain(|s, _| *s > seq);
                    self.my_sums.clear();
                    self.resyncing = false;
                    self.resyncs += 1;
                    info!("back in step from the host's game at command {seq}");
                }
                Err(e) => return Some(format!("the host's game didn't arrive whole: {e}")),
            },
            Msg::Refused { why } => return Some(why),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickets_round_trip() {
        let id = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::from_parts(id, [iroh::TransportAddr::Ip("192.168.1.20:4433".parse().unwrap())]);
        let t = encode_ticket(&addr);
        assert!(t.starts_with("torched"));
        assert_eq!(decode_ticket(&t).unwrap(), addr);
        assert!(decode_ticket("hello").is_err());
    }

    fn wait_for<T>(link: &Link, what: &str, mut f: impl FnMut(In) -> Option<T>) -> T {
        let start = std::time::Instant::now();
        loop {
            for ev in link.poll() {
                if let Some(t) = f(ev) {
                    return t;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A host and a peer on this machine play a whole game against each
    /// other over iroh, and end with the same game.
    #[test]
    fn two_peers_play_a_game_in_step() {
        play(false, false);
    }

    /// The peer runs at half speed, so the host's Advance reaches it while
    /// its last shell is still in the air (a slow or busy machine).
    #[test]
    fn a_slow_peer_keeps_in_step() {
        play(false, true);
    }

    /// The peer's game is knocked out of step; it notices, takes the
    /// host's game, and they finish the same.
    #[test]
    fn a_peer_out_of_step_recovers() {
        play(true, false);
    }

    fn play(knock: bool, slow: bool) {
        let host = Link::host(true);
        let ticket = wait_for(&host, "the ticket", |e| if let In::Ready { ticket } = e { Some(ticket) } else { None });
        let client = Link::join(&ticket, "Bea".into(), true).unwrap();
        let name = wait_for(&host, "the join", |e| if let In::Joined { name, .. } = e { Some(name) } else { None });
        assert_eq!(name, "Bea");
        let me = wait_for(&client, "the welcome", |e| match e {
            In::Got { msg: Msg::Welcome { peer }, .. } => Some(peer),
            In::Failed(f) => panic!("{f}"),
            _ => None,
        });
        assert_eq!(me, 1);

        let cfg = Config { players: 2, seed: 5, rounds: 2, free: true, ..Config::default() };
        let owners = vec![Owner::Peer(0), Owner::Peer(1)];
        let mut gh = crate::sim::tests::new_game(cfg.clone());
        let mut gc = crate::sim::tests::new_game(cfg);
        let mut sh = Session::new(host, 0, owners.clone());
        let mut sc = Session::new(client, 1, owners);
        // Each side acts once per (round, turn, phase): Advance can lead
        // straight into the next Aim, with no phase between to reset on.
        let mut fired = [false, false];
        let mut seen: [Option<(u32, usize, crate::sim::Phase)>; 2] = [None, None];
        let start = std::time::Instant::now();
        let mut shots = 0;
        let mut step = 0u64;
        while !(gh.phase == crate::sim::Phase::GameOver && gc.phase == crate::sim::Phase::GameOver) {
            assert!(start.elapsed() < Duration::from_secs(120), "game didn't finish: {shots} shots, round {}, host {:?}, peer {:?}", gh.round, gh.phase, gc.phase);
            step += 1;
            for (k, (g, s)) in [(&mut gh, &mut sh), (&mut gc, &mut sc)].into_iter().enumerate() {
                assert!(s.pump(g).is_none());
                if k == 1 && slow && step % 2 == 0 {
                    continue;
                }
                g.tick();
                use crate::sim::Phase;
                let now = Some((g.round, g.turn, g.phase));
                if seen[k] != now {
                    seen[k] = now;
                    fired[k] = false;
                }
                match g.phase {
                    Phase::Aim if k == 1 && knock && shots == 3 && g.tanks[0].money != 12345 => {
                        // Corrupt the peer's copy.
                        g.tanks[0].money = 12345;
                    }
                    Phase::Aim if s.is_local(g.turn) && !fired[k] => {
                        // Lob straight up: it lands on the shooter, so
                        // rounds end quickly.
                        let w = g.tanks[g.turn].weapon;
                        s.submit(g.turn, Cmd::Fire { angle: 90, power: 300, weapon: w });
                        fired[k] = true;
                        shots += 1;
                    }
                    // At GameOver the host waits, so both games end.
                    Phase::RoundOver if k == 0 && !fired[k] => {
                        s.submit(0, Cmd::Advance { round: g.round });
                        fired[k] = true;
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_micros(300));
        }
        assert_eq!(gh.checksum(), gc.checksum());
        assert!(shots >= 4, "{shots} shots");
        assert_ne!(gc.tanks[0].money, 12345);
        assert_eq!(sc.resyncs > 0, knock, "resyncs {}", sc.resyncs);
    }
}
