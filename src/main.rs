//! Torched: artillery on destructible ground, after Xscorch.

mod ai;
mod cmd;
mod defs;
mod economy;
mod fx;
mod input;
mod kit;
mod land;
mod menu;
mod net;
mod physics;
mod render;
mod sim;
mod sound;
mod weapons;

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use log::{LevelFilter, info};
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    execute,
};

use land::Generator;
use physics::Walls;
use economy::{Accessories, Scoring};
use sim::{Config, Game, Order, Phase};
use weapons::Arsenal;

const TICK: Duration = Duration::from_micros(16_667);

const USAGE: &str = "\
Torched: artillery on destructible ground

Usage: torched [options]

  --players N      people at this keyboard, taking turns (default 2,
                   or 0 with --ai or --demo)
  --ai LIST        computer players, comma-separated: moron, shooter,
                   spreader, chooser, calculater, annihilater, insanity,
                   random. Eight tanks at most.
  --demo           computer players only, the game plays itself
  --rounds N       rounds in a game (default 10)
  --terrain NAME   none, canyon, double-mountain, hillside, mountain,
                   plains, traditional, valley, or random (default)
  --bumpiness N    0 to 100 (default 50)
  --walls KIND     concrete, padded, rubber, spring, wrap (default),
                   none, or random
  --wind N         strongest wind, 0 to 10 (default 0.05)
  --steady-wind    wind stays the same all round
  --viscosity N    air drag, 0 to 0.2 (default 0)
  --sky KIND       night (default), fire or random
  --cash N         starting money (default 100000)
  --interest N     interest per round, 0 to 0.3 (default 0.05)
  --scoring NAME   basic, standard (default) or greedy
  --arms N         arms level, 0 to 6: what the shop sells (default 4)
  --order KIND     random (default), losers-first, winners-first,
                   round-robin
  --together       everyone aims, then all fire at once
  --tunneling      shells burrow through dirt while fast enough
  --free           every weapon free and endless, no shop
  --host           host a network game; others join with the ticket shown
  --seats N        people who may join over the network (default 1)
  --join TICKET    join a network game
  --name NAME      your name in a network game
  --lan            network game on this network only (no internet)
  --weapons FILE   weapons from a weapons.def file
  --accessories F  accessories from an accessories.def file
  --mute           no sound (m toggles it in play)
  --for SECONDS    quit by itself after this long (for recordings)
  --seed N         fix the random seed
  -h, --help       this help
  -V, --version    version

Keys: arrows aim (Shift for big steps), PgUp/PgDn power by 20,
      Tab and Shift+Tab pick a weapon, Space or Enter fires, q quits.
      s raises a shield, b uses a battery, c toggles contact triggers,
      a and d drive on fuel, x teleports.
Shop: up and down to choose, Enter or Right buys, Left sells, Tab
      switches between weapons and accessories, Space is done.";

struct Args {
    cfg: Config,
    mute: bool,
    quit_after: Option<f64>,
    /// Game options were given: start a game straight away, no title.
    direct: bool,
    weapons: Option<PathBuf>,
    accessories: Option<PathBuf>,
    net: Option<NetMode>,
    name: String,
    lan: bool,
}

#[derive(Clone)]
enum NetMode {
    Host { seats: usize },
    Join { ticket: String },
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut cfg = Config { seed: rand::random(), ..Config::default() };
    let mut direct = false;
    let mut weapons = None;
    let mut accessories = None;
    let mut players_given = false;
    let mut mute = false;
    let mut quit_after = None;
    let mut demo = false;
    let mut host = false;
    let mut seats = 1usize;
    let mut join = None;
    let mut name = std::env::var("USER").unwrap_or_else(|_| "Player".into());
    let mut lan = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if !matches!(a.as_str(), "--mute" | "--weapons" | "--accessories" | "--for" | "--name" | "--lan") {
            direct = true;
        }
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("torched {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--players" => {
                cfg.players = value(&a)?.parse().map_err(|_| "--players takes a number")?;
                players_given = true;
            }
            "--ai" => {
                for name in value(&a)?.split(',').filter(|n| !n.trim().is_empty()) {
                    cfg.ai.push(ai::AiKind::parse(name).ok_or_else(|| format!("unknown computer player '{name}'"))?);
                }
            }
            "--demo" => demo = true,
            "--mute" => mute = true,
            "--for" => quit_after = Some(value(&a)?.parse::<f64>().map_err(|_| "--for takes seconds")?),
            "--rounds" => {
                cfg.rounds = value(&a)?.parse().map_err(|_| "--rounds takes a number")?;
                if cfg.rounds == 0 {
                    return Err("--rounds must be at least 1".into());
                }
            }
            "--terrain" => {
                let v = value(&a)?;
                cfg.generator = if v.eq_ignore_ascii_case("random") {
                    None
                } else {
                    Some(Generator::parse(&v).ok_or_else(|| format!("unknown terrain '{v}'"))?)
                };
            }
            "--bumpiness" => {
                cfg.bumpiness = value(&a)?.parse().map_err(|_| "--bumpiness takes a number")?;
            }
            "--walls" => {
                let v = value(&a)?;
                cfg.walls = Walls::parse(&v).ok_or_else(|| format!("unknown walls '{v}'"))?;
            }
            "--wind" => {
                cfg.max_wind = value(&a)?.parse().map_err(|_| "--wind takes a number")?;
                if !(0.0..=physics::WIND_MAX).contains(&cfg.max_wind) {
                    return Err(format!("--wind must be 0 to {}", physics::WIND_MAX));
                }
            }
            "--steady-wind" => cfg.dynamic_wind = false,
            "--sky" => {
                let v = value(&a)?;
                cfg.sky = fx::Sky::parse(&v).ok_or_else(|| format!("unknown sky '{v}'"))?;
            }
            "--viscosity" => {
                cfg.viscosity = value(&a)?.parse().map_err(|_| "--viscosity takes a number")?;
                if !(0.0..=physics::VISCOSITY_MAX).contains(&cfg.viscosity) {
                    return Err(format!("--viscosity must be 0 to {}", physics::VISCOSITY_MAX));
                }
            }
            "--weapons" => weapons = Some(PathBuf::from(value(&a)?)),
            "--accessories" => accessories = Some(PathBuf::from(value(&a)?)),
            "--cash" => {
                cfg.cash = value(&a)?.parse().map_err(|_| "--cash takes a number")?;
                if !(0..=economy::MAX_CASH).contains(&cfg.cash) {
                    return Err(format!("--cash must be 0 to {}", economy::MAX_CASH));
                }
            }
            "--interest" => {
                cfg.interest = value(&a)?.parse().map_err(|_| "--interest takes a number")?;
                if !(0.0..=economy::MAX_INTEREST).contains(&cfg.interest) {
                    return Err(format!("--interest must be 0 to {}", economy::MAX_INTEREST));
                }
            }
            "--scoring" => cfg.scoring = value(&a)?,
            "--arms" => {
                cfg.arms_level = value(&a)?.parse().map_err(|_| "--arms takes a number")?;
                if cfg.arms_level > economy::ARMS_LEVEL_MAX {
                    return Err(format!("--arms must be 0 to {}", economy::ARMS_LEVEL_MAX));
                }
            }
            "--order" => {
                let v = value(&a)?;
                cfg.order = Order::parse(&v).ok_or_else(|| format!("unknown order '{v}'"))?;
            }
            "--together" => cfg.simultaneous = true,
            "--tunneling" | "--tunnelling" => cfg.tunneling = true,
            "--free" => cfg.free = true,
            "--host" => host = true,
            "--seats" => {
                seats = value(&a)?.parse().map_err(|_| "--seats takes a number")?;
                if !(1..sim::MAX_TANKS).contains(&seats) {
                    return Err(format!("--seats must be 1 to {}", sim::MAX_TANKS - 1));
                }
            }
            "--join" => join = Some(value(&a)?),
            "--name" => name = value(&a)?,
            "--lan" => lan = true,
            "--seed" => cfg.seed = value(&a)?.parse().map_err(|_| "--seed takes a number")?,
            other => return Err(format!("unknown option '{other}'\n\n{USAGE}")),
        }
    }
    if demo {
        cfg.players = 0;
        if cfg.ai.is_empty() {
            // The sharper ones make a better show.
            cfg.ai = vec![ai::AiKind::Chooser, ai::AiKind::Calculater, ai::AiKind::Annihilater, ai::AiKind::Insanity];
        }
    } else if !players_given && !cfg.ai.is_empty() {
        // `--ai` alone: one person against the computers.
        cfg.players = 1;
    }
    let net = match (host, join) {
        (true, Some(_)) => return Err("--host and --join don't go together".into()),
        (true, None) => {
            if !players_given {
                cfg.players = 1;
            }
            Some(NetMode::Host { seats })
        }
        (false, Some(ticket)) => Some(NetMode::Join { ticket }),
        _ => None,
    };
    if net.is_some() {
        // Over the network turns come one at a time; all-at-once would
        // need everyone's orders before anyone's shot.
        cfg.simultaneous = false;
    }
    let total = cfg.players + cfg.ai.len() + if let Some(NetMode::Host { seats }) = net { seats } else { 0 };
    if direct && !matches!(net, Some(NetMode::Join { .. })) && !(2..=sim::MAX_TANKS).contains(&total) {
        return Err(format!("a game needs 2 to {} tanks, people and computers together (got {total})", sim::MAX_TANKS));
    }
    Ok(Some(Args { cfg, mute, quit_after, direct, weapons, accessories, net, name, lan }))
}

fn load_scoring(name: &str) -> Result<Scoring, String> {
    let all = Scoring::parse_all(economy::DEFAULT_SCORINGS)?;
    let names: Vec<String> = all.iter().map(|s| s.name.to_lowercase()).collect();
    all.into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("unknown scoring '{name}' (try {})", names.join(", ")))
}

/// Session log in the state folder; the terminal is the game's.
fn init_logging() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?
        .join("torched");
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join("session.log");
    let file = fs::File::create(&path).ok()?;
    env_logger::Builder::new()
        .filter_level(LevelFilter::Info)
        .parse_env("TORCHED_LOG")
        .target(env_logger::Target::Pipe(Box::new(file)))
        .format(|buf, rec| writeln!(buf, "{} {:<5} {}", buf.timestamp_millis(), rec.level(), rec.args()))
        .init();
    Some(path)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("torched: {e}");
            return ExitCode::FAILURE;
        }
    };
    let data = match Data::load(&args) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("torched: {e}");
            return ExitCode::FAILURE;
        }
    };
    let log_path = init_logging();
    info!("torched {} start, {:?}", env!("CARGO_PKG_VERSION"), args.cfg);

    let opts = menu::Options::load();
    // Sound opens before the screen is taken, so any complaint from the
    // sound system lands in the shell, not over the game.
    let mut audio = sound::Audio::new(args.mute || (!args.direct && !opts.sound));
    let app = if let Some(mode) = args.net.clone() {
        App::lobby(&data, opts, &args, mode)
    } else if args.direct {
        match data.game(args.cfg.clone()) {
            Ok(g) => App::direct(g, &data, opts),
            Err(e) => {
                eprintln!("torched: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        App::title(&data, opts)
    };
    let mut pads = input::Pads::new();
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let result = run(&mut terminal, app, &data, &mut audio, &mut pads, args.quit_after);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    info!("exit");

    if let Err(e) = result {
        eprintln!("torched: {e}");
        if let Some(p) = log_path {
            eprintln!("session log: {}", p.display());
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The data files, read once; each game gets fresh copies.
struct Data {
    weapons: String,
    accessories: String,
}

impl Data {
    fn load(args: &Args) -> Result<Data, String> {
        let read = |p: &Option<PathBuf>, builtin: &str| -> Result<String, String> {
            match p {
                None => Ok(builtin.to_string()),
                Some(p) => fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())),
            }
        };
        let d = Data {
            weapons: read(&args.weapons, weapons::DEFAULT_DEF)?,
            accessories: read(&args.accessories, economy::DEFAULT_ACCESSORIES)?,
        };
        // Check them now, not when a game starts.
        Arsenal::parse(&d.weapons).map_err(|e| format!("weapons: {e}"))?;
        Accessories::parse(&d.accessories).map_err(|e| format!("accessories: {e}"))?;
        load_scoring(&args.cfg.scoring)?;
        Ok(d)
    }

    fn game(&self, cfg: Config) -> Result<Game, String> {
        let scoring = load_scoring(&cfg.scoring)?;
        Ok(Game::new(cfg, Arsenal::parse(&self.weapons)?, Accessories::parse(&self.accessories)?, scoring))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Title,
    Options,
    Lobby,
    Playing,
}

/// Before a network game: the link, the ticket, who has joined.
struct Lobby {
    link: Option<net::Link>,
    host: bool,
    lan: bool,
    ticket: Option<String>,
    /// Host first, then those who joined.
    names: Vec<String>,
    /// Peer numbers of those who joined, in order.
    peers: Vec<u8>,
    me: u8,
    seats: usize,
    cfg: Config,
    status: String,
}

struct App {
    screen: Screen,
    opts: menu::Options,
    /// Computers playing behind the title and options.
    bg: Game,
    game: Option<Game>,
    /// Started from the command line: quitting the game quits.
    direct: bool,
    cursor: usize,
    note: String,
    lobby: Option<Lobby>,
    /// The network game's lockstep, when playing over the network.
    net: Option<net::Session>,
}

impl App {
    fn title(data: &Data, opts: menu::Options) -> App {
        let bg = data.game(menu::title_game_config(rand::random())).expect("built-in data loads");
        App { screen: Screen::Title, opts, bg, game: None, direct: false, cursor: 0, note: String::new(), lobby: None, net: None }
    }

    fn lobby(data: &Data, opts: menu::Options, args: &Args, mode: NetMode) -> App {
        let mut app = App::title(data, opts);
        app.screen = Screen::Lobby;
        app.direct = true;
        let (link, host, seats, status) = match mode {
            NetMode::Host { seats } => (Some(net::Link::host(args.lan)), true, seats, String::new()),
            NetMode::Join { ticket } => match net::Link::join(&ticket, args.name.clone(), args.lan) {
                Ok(l) => (Some(l), false, 0, "Connecting to the host...".into()),
                Err(e) => (None, false, 0, e),
            },
        };
        app.lobby = Some(Lobby {
            link,
            host,
            lan: args.lan,
            ticket: None,
            names: if host { vec![args.name.clone()] } else { Vec::new() },
            peers: Vec::new(),
            me: 0,
            seats,
            cfg: args.cfg.clone(),
            status,
        });
        app
    }

    /// Leave a game, or the lobby, for the title (or quit if started from
    /// the command line).
    fn end_game(&mut self, note: String) -> bool {
        self.game = None;
        self.net = None;
        self.lobby = None;
        self.note = note;
        self.screen = Screen::Title;
        self.cursor = 0;
        !self.direct || !self.note.is_empty()
    }

    /// A player's command: applied now at one keyboard, or put in line
    /// for everyone over the network.
    fn submit(&mut self, tank: usize, cmd: cmd::Cmd) {
        if let Some(s) = self.net.as_mut() {
            s.submit(tank, cmd);
        } else if let Some(g) = self.game.as_mut()
            && g.can_apply(tank, &cmd)
        {
            g.apply(tank, &cmd);
        }
    }

    fn direct(game: Game, data: &Data, opts: menu::Options) -> App {
        let mut app = App::title(data, opts);
        app.screen = Screen::Playing;
        app.game = Some(game);
        app.direct = true;
        app
    }
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    data: &Data,
    audio: &mut sound::Audio,
    pads: &mut input::Pads,
    quit_after: Option<f64>,
) -> io::Result<()> {
    let start = Instant::now();
    let mut next = Instant::now();
    loop {
        if quit_after.is_some_and(|s| start.elapsed().as_secs_f64() >= s) {
            return Ok(());
        }
        terminal.draw(|f| match (app.screen, &app.game) {
            (Screen::Playing, Some(g)) => render::draw(f, g),
            (Screen::Options, _) => render::draw_options(f, &app.bg, &app.opts.rows(), app.cursor),
            (Screen::Lobby, _) => {
                if let Some(l) = &app.lobby {
                    let v = render::LobbyView {
                        host: l.host,
                        ticket: l.ticket.as_deref(),
                        names: &l.names,
                        seats: l.seats,
                        status: &l.status,
                        lan: l.lan,
                    };
                    render::draw_lobby(f, &app.bg, &v);
                }
            }
            _ => render::draw_title(f, &app.bg, app.cursor, &app.opts.summary(), &app.note),
        })?;
        next += TICK;
        let now = Instant::now();
        if next < now {
            next = now;
        }
        while event::poll(next.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if !dispatch(&mut app, k, data, audio) {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => {
                    let size = terminal.size()?;
                    if let Some(k) = mouse(&mut app, m, (size.width, size.height))
                        && !dispatch(&mut app, k, data, audio)
                    {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        let ctx = match (app.screen, &app.game) {
            (Screen::Playing, Some(g)) if g.phase == Phase::Shop => input::Context::Shop,
            (Screen::Playing, _) => input::Context::Play,
            _ => input::Context::Menu,
        };
        for k in pads.poll(ctx) {
            if !dispatch(&mut app, k, data, audio) {
                return Ok(());
            }
        }
        // The background game plays on, silently, behind the menus.
        app.bg.tick();
        app.bg.fx.sounds.clear();
        if app.screen == Screen::Lobby {
            lobby_tick(&mut app);
        }
        if let (Some(s), Some(g)) = (app.net.as_mut(), app.game.as_mut())
            && let Some(end) = s.pump(g)
        {
            app.end_game(end);
        }
        if let (Some(s), Some(g)) = (app.net.as_mut(), app.game.as_ref()) {
            s.share_aim(g);
        }
        if let Some(g) = app.game.as_mut() {
            g.tick();
            for s in std::mem::take(&mut g.fx.sounds) {
                audio.play(s);
            }
        }
    }
}

/// A key from the keyboard or a gamepad, for whichever screen is up.
/// False to quit.
fn dispatch(app: &mut App, k: KeyEvent, data: &Data, audio: &mut sound::Audio) -> bool {
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    match app.screen {
        Screen::Playing => {
            if k.code == KeyCode::Char('m') {
                audio.toggle();
                return true;
            }
            let Some(g) = app.game.as_mut() else { return true };
            match key(g, k) {
                Key::Quit => {
                    if app.direct {
                        return false;
                    }
                    app.end_game(String::new());
                }
                Key::Cmd(tank, c) => app.submit(tank, c),
                Key::Nothing => {}
            }
            true
        }
        Screen::Lobby => match k.code {
            KeyCode::Char('q') | KeyCode::Esc => false,
            KeyCode::Enter | KeyCode::Char(' ') => {
                start_hosted(app, data);
                true
            }
            _ => true,
        },
        Screen::Title => title_key(app, k, data, audio),
        Screen::Options => {
            options_key(app, k, audio);
            true
        }
    }
}

/// The mouse: in play, press or drag on the field to aim (direction and
/// distance set angle and power), wheel for fine power, right button to
/// fire. In the shop and menus, the wheel moves, left chooses or buys,
/// right sells. Returns a key to act on, when it acts like one.
fn mouse(app: &mut App, m: MouseEvent, (w, h): (u16, u16)) -> Option<KeyEvent> {
    let key_of = |code| Some(KeyEvent::new(code, KeyModifiers::NONE));
    match app.screen {
        Screen::Playing => {
            let g = app.game.as_mut()?;
            if g.phase == Phase::Shop {
                return match m.kind {
                    MouseEventKind::ScrollUp => key_of(KeyCode::Up),
                    MouseEventKind::ScrollDown => key_of(KeyCode::Down),
                    MouseEventKind::Down(MouseButton::Left) => key_of(KeyCode::Enter),
                    MouseEventKind::Down(MouseButton::Right) => key_of(KeyCode::Left),
                    _ => None,
                };
            }
            if matches!(g.phase, Phase::RoundOver | Phase::GameOver) {
                return matches!(m.kind, MouseEventKind::Down(_)).then(|| KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            if g.phase != Phase::Aim || g.is_ai(g.turn) || g.is_remote(g.turn) {
                return None;
            }
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                    // The field is the screen between the two HUD rows and
                    // the footer, as render::draw lays it out.
                    let (top, rows) = (2.0, h.saturating_sub(3).max(1) as f64);
                    let wx = (m.column as f64 + 0.5) / w.max(1) as f64 * land::WIDTH as f64;
                    let wy = land::HEIGHT as f64 - (m.row as f64 - top + 0.5) / rows * land::HEIGHT as f64;
                    g.aim_at_point(wx, wy);
                    None
                }
                MouseEventKind::ScrollUp => key_of(KeyCode::Up),
                MouseEventKind::ScrollDown => key_of(KeyCode::Down),
                MouseEventKind::Down(MouseButton::Right) => key_of(KeyCode::Char(' ')),
                _ => None,
            }
        }
        Screen::Title | Screen::Options => {
            let code = match m.kind {
                MouseEventKind::ScrollUp => KeyCode::Up,
                MouseEventKind::ScrollDown => KeyCode::Down,
                MouseEventKind::Down(MouseButton::Left) => KeyCode::Enter,
                _ => return None,
            };
            if app.screen == Screen::Options {
                options_key_simple(app, KeyEvent::new(code, KeyModifiers::NONE));
            } else if code != KeyCode::Enter {
                app.cursor = match code {
                    KeyCode::Up => (app.cursor + TITLE_ITEMS - 1) % TITLE_ITEMS,
                    _ => (app.cursor + 1) % TITLE_ITEMS,
                };
            }
            None
        }
        Screen::Lobby => None,
    }
}

/// Network news while in the lobby.
fn lobby_tick(app: &mut App) {
    let Some(l) = app.lobby.as_mut() else { return };
    let Some(link) = l.link.as_ref() else { return };
    let mut start = None;
    for ev in link.poll() {
        match ev {
            net::In::Ready { ticket } => {
                if let Some(dir) = std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
                {
                    let _ = fs::write(dir.join("torched").join("ticket"), format!("{ticket}\n"));
                }
                l.ticket = Some(ticket);
            }
            net::In::Joined { peer, name } => {
                if l.peers.len() >= l.seats {
                    link.to(peer, net::Msg::Refused { why: "the game is full".into() });
                } else {
                    info!("{name} joined as peer {peer}");
                    l.peers.push(peer);
                    l.names.push(name);
                    link.all(net::Msg::Lobby { names: l.names.clone() });
                }
            }
            net::In::Left { peer } => {
                if l.host {
                    if let Some(i) = l.peers.iter().position(|&p| p == peer) {
                        l.peers.remove(i);
                        l.names.remove(i + 1);
                        link.all(net::Msg::Lobby { names: l.names.clone() });
                    }
                } else {
                    l.status = "The host went away.".into();
                }
            }
            net::In::Got { msg, .. } => match msg {
                net::Msg::Welcome { peer } => {
                    l.me = peer;
                    l.status = "Joined. Waiting for the host to start.".into();
                }
                net::Msg::Lobby { names } => l.names = names,
                net::Msg::Refused { why } => l.status = format!("The host said no: {why}"),
                net::Msg::Start { cfg, weapons, accessories, names, owners } => start = Some((cfg, weapons, accessories, names, owners)),
                _ => {}
            },
            net::In::Failed(e) => l.status = e,
        }
    }
    if let Some((cfg, weapons, accessories, names, owners)) = start {
        let data = Data { weapons, accessories };
        let link = l.link.take().expect("a link");
        let me = l.me;
        begin_network_game(app, &data, cfg, names, owners, link, me);
    }
}

/// The host starts: those who joined get a tank each, empty seats go to
/// computers, and everyone gets the game.
fn start_hosted(app: &mut App, data: &Data) {
    let Some(l) = app.lobby.as_mut() else { return };
    if !l.host || l.link.is_none() {
        return;
    }
    let mut cfg = l.cfg.clone();
    cfg.seed = rand::random();
    let local = cfg.players;
    let empty = l.seats - l.peers.len();
    cfg.players = local + l.peers.len();
    // Empty seats get computers, after the people.
    let mut ai = vec![ai::AiKind::Chooser; empty];
    ai.extend(cfg.ai.iter().copied());
    cfg.ai = ai;
    if !(2..=sim::MAX_TANKS).contains(&(cfg.players + cfg.ai.len())) {
        l.status = format!("A game needs 2 to {} tanks.", sim::MAX_TANKS);
        return;
    }
    let mut owners = vec![net::Owner::Peer(0); local];
    owners.extend(l.peers.iter().map(|&p| net::Owner::Peer(p)));
    owners.extend(std::iter::repeat_n(net::Owner::Computer, cfg.ai.len()));
    let host_name = l.names[0].clone();
    let mut names: Vec<String> =
        (0..local).map(|i| if local > 1 { format!("{host_name} {}", i + 1) } else { host_name.clone() }).collect();
    names.extend(l.names[1..].iter().cloned());
    let link = l.link.take().expect("a link");
    link.all(net::Msg::Start {
        cfg: cfg.clone(),
        weapons: data.weapons.clone(),
        accessories: data.accessories.clone(),
        names: names.clone(),
        owners: owners.clone(),
    });
    begin_network_game(app, data, cfg, names, owners, link, 0);
}

fn begin_network_game(
    app: &mut App,
    data: &Data,
    cfg: Config,
    names: Vec<String>,
    owners: Vec<net::Owner>,
    link: net::Link,
    me: u8,
) {
    match data.game(cfg) {
        Ok(mut g) => {
            for (t, n) in g.tanks.iter_mut().zip(&names) {
                t.name = n.clone();
            }
            g.remote = owners.iter().map(|o| matches!(o, net::Owner::Peer(p) if *p != me)).collect();
            info!("network game starts, peer {me}, owners {owners:?}");
            app.game = Some(g);
            app.net = Some(net::Session::new(link, me, owners));
            app.lobby = None;
            app.screen = Screen::Playing;
        }
        Err(e) => {
            if let Some(l) = app.lobby.as_mut() {
                l.status = e;
            }
        }
    }
}

/// Options moves that need no sound handle (mouse).
fn options_key_simple(app: &mut App, k: KeyEvent) {
    let rows = app.opts.rows().len();
    match k.code {
        KeyCode::Up => app.cursor = (app.cursor + rows - 1) % rows,
        KeyCode::Down => app.cursor = (app.cursor + 1) % rows,
        KeyCode::Enter if !app.opts.is_back(app.cursor) => app.opts.change(app.cursor, 1),
        _ => {}
    }
}

const TITLE_ITEMS: usize = 3;

/// Title screen keys: Play, Options, Quit. False to quit.
fn title_key(app: &mut App, k: KeyEvent, data: &Data, audio: &mut sound::Audio) -> bool {
    match k.code {
        KeyCode::Up => app.cursor = (app.cursor + TITLE_ITEMS - 1) % TITLE_ITEMS,
        KeyCode::Down | KeyCode::Tab => app.cursor = (app.cursor + 1) % TITLE_ITEMS,
        KeyCode::Char('q') | KeyCode::Esc => return false,
        KeyCode::Char('o') => {
            app.screen = Screen::Options;
            app.cursor = 0;
        }
        KeyCode::Enter | KeyCode::Char(' ') => match app.cursor {
            0 => {
                let n = app.opts.tanks();
                if !(2..=sim::MAX_TANKS).contains(&n) {
                    app.note = format!("A game needs 2 to {} tanks; set people and computers in Options.", sim::MAX_TANKS);
                    return true;
                }
                match data.game(app.opts.to_config(rand::random())) {
                    Ok(g) => {
                        info!("new game: {}", app.opts.summary());
                        audio.muted = !app.opts.sound;
                        app.game = Some(g);
                        app.screen = Screen::Playing;
                        app.note.clear();
                    }
                    Err(e) => app.note = e,
                }
            }
            1 => {
                app.screen = Screen::Options;
                app.cursor = 0;
            }
            _ => return false,
        },
        _ => {}
    }
    true
}

/// Options screen keys: up and down to pick, left and right to change.
fn options_key(app: &mut App, k: KeyEvent, audio: &mut sound::Audio) {
    let rows = app.opts.rows().len();
    let leave = |app: &mut App, audio: &mut sound::Audio| {
        app.opts.save();
        audio.muted = !app.opts.sound;
        app.screen = Screen::Title;
        app.cursor = 1;
    };
    match k.code {
        KeyCode::Up => app.cursor = (app.cursor + rows - 1) % rows,
        KeyCode::Down | KeyCode::Tab => app.cursor = (app.cursor + 1) % rows,
        KeyCode::PageUp => app.cursor = app.cursor.saturating_sub(8),
        KeyCode::PageDown => app.cursor = (app.cursor + 8).min(rows - 1),
        KeyCode::Left => app.opts.change(app.cursor, -1),
        KeyCode::Right => app.opts.change(app.cursor, 1),
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.opts.is_back(app.cursor) {
                leave(app, audio);
            } else {
                app.opts.change(app.cursor, 1);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => leave(app, audio),
        _ => {}
    }
    // Rows come and go with the number of computers.
    app.cursor = app.cursor.min(app.opts.rows().len() - 1);
}

enum Key {
    Quit,
    Nothing,
    Cmd(usize, cmd::Cmd),
}

/// What a key means in play. Turning the dial and browsing the shop
/// happen here and now; anything that changes the game comes back as a
/// command, for whoever plays the tank.
fn key(game: &mut Game, k: KeyEvent) -> Key {
    use cmd::Cmd;
    if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
        return Key::Quit;
    }
    if matches!(game.phase, Phase::RoundOver | Phase::GameOver) {
        return if matches!(k.code, KeyCode::Enter | KeyCode::Char(' ')) {
            Key::Cmd(0, Cmd::Advance { round: game.round })
        } else {
            Key::Nothing
        };
    }
    let who = match game.phase {
        Phase::Shop => game.shop.player,
        Phase::Aim => game.turn,
        _ => return Key::Nothing,
    };
    // Computers and other machines' players act for themselves.
    if game.is_ai(who) || game.is_remote(who) {
        return Key::Nothing;
    }
    if game.phase == Phase::Shop {
        let item = || game.aisle(game.shop.aisle).get(game.shop.cursor).copied();
        match k.code {
            KeyCode::Up => game.shop_move(-1),
            KeyCode::Down => game.shop_move(1),
            KeyCode::PageUp => game.shop_move(-10),
            KeyCode::PageDown => game.shop_move(10),
            KeyCode::Tab | KeyCode::BackTab => game.shop_switch(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('+') => {
                if let Some(item) = item() {
                    return Key::Cmd(who, Cmd::Buy { aisle: game.shop.aisle, item });
                }
            }
            KeyCode::Left | KeyCode::Char('-') => {
                if let Some(item) = item() {
                    return Key::Cmd(who, Cmd::Sell { aisle: game.shop.aisle, item });
                }
            }
            KeyCode::Char(' ') => return Key::Cmd(who, Cmd::ShopDone),
            _ => {}
        }
        return Key::Nothing;
    }
    let big = k.modifiers.contains(KeyModifiers::SHIFT);
    let (turret, power) = if big { (sim::TURRET_BIGSTEP, sim::POWER_BIGSTEP) } else { (sim::TURRET_STEP, sim::POWER_STEP) };
    match k.code {
        // Angle 0 points right, so Left turns the barrel up and over.
        KeyCode::Left => game.adjust_angle(turret),
        KeyCode::Right => game.adjust_angle(-turret),
        KeyCode::Up => game.adjust_power(power),
        KeyCode::Down => game.adjust_power(-power),
        KeyCode::PageUp => game.adjust_power(sim::POWER_BIGSTEP),
        KeyCode::PageDown => game.adjust_power(-sim::POWER_BIGSTEP),
        KeyCode::Tab | KeyCode::Char(']') => game.cycle_weapon(1),
        KeyCode::BackTab | KeyCode::Char('[') => game.cycle_weapon(-1),
        KeyCode::Char('s') => return Key::Cmd(who, Cmd::Shield),
        KeyCode::Char('b') => return Key::Cmd(who, Cmd::Battery),
        KeyCode::Char('c') => return Key::Cmd(who, Cmd::Triggers(!game.tanks[who].triggers)),
        KeyCode::Char('a') => return Key::Cmd(who, Cmd::Drive(-1)),
        KeyCode::Char('d') => return Key::Cmd(who, Cmd::Drive(1)),
        KeyCode::Char('x') => return Key::Cmd(who, Cmd::Teleport),
        KeyCode::Char(' ') | KeyCode::Enter => return Key::Cmd(who, game.aim_of(who, true)),
        _ => {}
    }
    Key::Nothing
}
