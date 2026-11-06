//! Torched: artillery on destructible ground, after Xscorch.

mod land;
mod render;
mod sim;

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use log::{LevelFilter, info};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use land::Generator;
use sim::{Config, Game, Phase};

const TICK: Duration = Duration::from_micros(16_667);

const USAGE: &str = "\
Torched: artillery on destructible ground

Usage: torched [options]

  --players N      tanks, hot-seat, 2 to 4 (default 2)
  --rounds N       rounds in a game (default 10)
  --terrain NAME   none, canyon, double-mountain, hillside, mountain,
                   plains, traditional, valley, or random (default)
  --bumpiness N    0 to 100 (default 50)
  --seed N         fix the random seed
  -h, --help       this help
  -V, --version    version

Keys: arrows aim (Shift for big steps), PgUp/PgDn power by 20,
      Space or Enter fires, q quits.";

fn parse_args() -> Result<Option<Config>, String> {
    let mut cfg = Config {
        players: 2,
        rounds: 10,
        generator: None,
        bumpiness: land::BUMPINESS_DEF,
        seed: rand::random(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
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
                if !(2..=4).contains(&cfg.players) {
                    return Err("--players must be 2 to 4".into());
                }
            }
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
            "--seed" => cfg.seed = value(&a)?.parse().map_err(|_| "--seed takes a number")?,
            other => return Err(format!("unknown option '{other}'\n\n{USAGE}")),
        }
    }
    Ok(Some(cfg))
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
    let cfg = match parse_args() {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("torched: {e}");
            return ExitCode::FAILURE;
        }
    };
    let log_path = init_logging();
    info!("torched {} start, {:?}", env!("CARGO_PKG_VERSION"), cfg);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, Game::new(cfg));
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

fn run(terminal: &mut ratatui::DefaultTerminal, mut game: Game) -> io::Result<()> {
    let mut next = Instant::now();
    loop {
        terminal.draw(|f| render::draw(f, &game))?;
        next += TICK;
        let now = Instant::now();
        if next < now {
            next = now;
        }
        while event::poll(next.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if !key(&mut game, k) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        game.tick();
    }
}

/// Handle a key; false to quit.
fn key(game: &mut Game, k: KeyEvent) -> bool {
    let big = k.modifiers.contains(KeyModifiers::SHIFT);
    let (turret, power) = if big { (sim::TURRET_BIGSTEP, sim::POWER_BIGSTEP) } else { (sim::TURRET_STEP, sim::POWER_STEP) };
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => return false,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return false,
        // Angle 0 points right, so Left turns the barrel up and over.
        KeyCode::Left => game.adjust_angle(turret),
        KeyCode::Right => game.adjust_angle(-turret),
        KeyCode::Up => game.adjust_power(power),
        KeyCode::Down => game.adjust_power(-power),
        KeyCode::PageUp => game.adjust_power(sim::POWER_BIGSTEP),
        KeyCode::PageDown => game.adjust_power(-sim::POWER_BIGSTEP),
        KeyCode::Char(' ') | KeyCode::Enter => match game.phase {
            Phase::Aim => game.fire(),
            Phase::RoundOver | Phase::GameOver => game.advance(),
            _ => {}
        },
        _ => {}
    }
    true
}
