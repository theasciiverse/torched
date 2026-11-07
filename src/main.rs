//! Torched: artillery on destructible ground, after Xscorch.

mod ai;
mod defs;
mod economy;
mod fx;
mod input;
mod kit;
mod land;
mod menu;
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
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if !matches!(a.as_str(), "--mute" | "--weapons" | "--accessories" | "--for") {
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
    let total = cfg.players + cfg.ai.len();
    if direct && !(2..=sim::MAX_TANKS).contains(&total) {
        return Err(format!("a game needs 2 to {} tanks, people and computers together (got {total})", sim::MAX_TANKS));
    }
    Ok(Some(Args { cfg, mute, quit_after, direct, weapons, accessories }))
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
    let app = if args.direct {
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
    Playing,
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
}

impl App {
    fn title(data: &Data, opts: menu::Options) -> App {
        let bg = data.game(menu::title_game_config(rand::random())).expect("built-in data loads");
        App { screen: Screen::Title, opts, bg, game: None, direct: false, cursor: 0, note: String::new() }
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
                    mouse(&mut app, m, (size.width, size.height));
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
            } else if let Some(g) = app.game.as_mut()
                && key(g, k)
            {
            } else if app.direct {
                return false;
            } else {
                // Back to the title from a game.
                app.game = None;
                app.screen = Screen::Title;
                app.cursor = 0;
            }
            true
        }
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
/// right sells.
fn mouse(app: &mut App, m: MouseEvent, (w, h): (u16, u16)) {
    let key_of = |code| KeyEvent::new(code, KeyModifiers::NONE);
    match app.screen {
        Screen::Playing => {
            let Some(g) = app.game.as_mut() else { return };
            if g.phase == Phase::Shop {
                match m.kind {
                    MouseEventKind::ScrollUp => g.shop_move(-1),
                    MouseEventKind::ScrollDown => g.shop_move(1),
                    MouseEventKind::Down(MouseButton::Left) => g.shop_buy(),
                    MouseEventKind::Down(MouseButton::Right) => g.shop_sell(),
                    _ => {}
                }
                return;
            }
            if g.phase != Phase::Aim || g.is_ai(g.turn) {
                if matches!(m.kind, MouseEventKind::Down(_)) && matches!(g.phase, Phase::RoundOver | Phase::GameOver) {
                    g.advance();
                }
                return;
            }
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                    // The field is the screen between the two HUD rows and
                    // the footer, as render::draw lays it out.
                    let (top, rows) = (2.0, h.saturating_sub(3).max(1) as f64);
                    let wx = (m.column as f64 + 0.5) / w.max(1) as f64 * land::WIDTH as f64;
                    let wy = land::HEIGHT as f64 - (m.row as f64 - top + 0.5) / rows * land::HEIGHT as f64;
                    g.aim_at_point(wx, wy);
                }
                MouseEventKind::ScrollUp => g.adjust_power(5),
                MouseEventKind::ScrollDown => g.adjust_power(-5),
                MouseEventKind::Down(MouseButton::Right) => g.fire(),
                _ => {}
            }
        }
        Screen::Title | Screen::Options => {
            let code = match m.kind {
                MouseEventKind::ScrollUp => KeyCode::Up,
                MouseEventKind::ScrollDown => KeyCode::Down,
                MouseEventKind::Down(MouseButton::Left) => KeyCode::Enter,
                _ => return,
            };
            if app.screen == Screen::Options {
                options_key_simple(app, key_of(code));
            } else if code != KeyCode::Enter {
                app.cursor = match code {
                    KeyCode::Up => (app.cursor + TITLE_ITEMS - 1) % TITLE_ITEMS,
                    _ => (app.cursor + 1) % TITLE_ITEMS,
                };
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

/// Handle a key; false to quit.
fn key(game: &mut Game, k: KeyEvent) -> bool {
    if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
    {
        return false;
    }
    // Computers shop and aim by themselves; Enter still moves past results.
    let computer = match game.phase {
        Phase::Shop => game.is_ai(game.shop.player),
        Phase::Aim => game.is_ai(game.turn),
        _ => false,
    };
    if computer {
        return true;
    }
    if game.phase == Phase::Shop {
        match k.code {
            KeyCode::Up => game.shop_move(-1),
            KeyCode::Down => game.shop_move(1),
            KeyCode::PageUp => game.shop_move(-10),
            KeyCode::PageDown => game.shop_move(10),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('+') => game.shop_buy(),
            KeyCode::Left | KeyCode::Char('-') => game.shop_sell(),
            KeyCode::Tab | KeyCode::BackTab => game.shop_switch(),
            KeyCode::Char(' ') => game.shop_done(),
            _ => {}
        }
        return true;
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
        KeyCode::Char('s') => {
            game.raise_shield();
        }
        KeyCode::Char('b') => {
            game.use_battery();
        }
        KeyCode::Char('c') => game.toggle_triggers(),
        KeyCode::Char('a') => {
            game.drive(-1);
        }
        KeyCode::Char('d') => {
            game.drive(1);
        }
        KeyCode::Char('x') => {
            game.teleport();
        }
        KeyCode::Char(' ') | KeyCode::Enter => match game.phase {
            Phase::Aim => game.fire(),
            Phase::RoundOver | Phase::GameOver => game.advance(),
            _ => {}
        },
        _ => {}
    }
    true
}
