//! Gamepads, through gilrs, turned into the keys the game already knows.
//! No pad, or no way to read pads, and this does nothing.

use gilrs::{Axis, Button, EventType, Gilrs};
use log::{info, warn};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

/// What the keys mean depends on where we are.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Menu,
    Shop,
    Play,
}

/// A held direction repeats after this many ticks, then every few.
const REPEAT_AFTER: u32 = 14;
const REPEAT_EVERY: u32 = 3;
const STICK: f32 = 0.5;

pub struct Pads {
    gilrs: Option<Gilrs>,
    /// Held directions: left, right, up, down, and ticks held.
    held: [Option<u32>; 4],
    stick: (f32, f32),
}

impl Pads {
    pub fn new() -> Pads {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                for (_, pad) in g.gamepads() {
                    info!("gamepad: {}", pad.name());
                }
                Some(g)
            }
            Err(e) => {
                warn!("no gamepads: {e}");
                None
            }
        };
        Pads { gilrs, held: [None; 4], stick: (0.0, 0.0) }
    }

    /// Keys pressed on any pad since the last tick, and repeats of held
    /// directions.
    pub fn poll(&mut self, ctx: Context) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        let Some(g) = self.gilrs.as_mut() else { return out };
        while let Some(ev) = g.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(d) = dir_of(b) {
                        self.held[d] = Some(0);
                        out.push(dir_key(d));
                    } else if let Some(k) = button_key(b, ctx) {
                        out.push(k);
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if let Some(d) = dir_of(b) {
                        self.held[d] = None;
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => self.stick.0 = v,
                EventType::AxisChanged(Axis::LeftStickY, v, _) => self.stick.1 = v,
                EventType::Connected => info!("gamepad connected"),
                _ => {}
            }
        }
        // The stick acts as a d-pad.
        let want = [self.stick.0 < -STICK, self.stick.0 > STICK, self.stick.1 > STICK, self.stick.1 < -STICK];
        for (d, on) in want.into_iter().enumerate() {
            if on && self.held[d].is_none() {
                self.held[d] = Some(0);
                out.push(dir_key(d));
            }
        }
        for (d, h) in self.held.iter_mut().enumerate() {
            if let Some(t) = h {
                *t += 1;
                if *t >= REPEAT_AFTER && (*t - REPEAT_AFTER).is_multiple_of(REPEAT_EVERY) {
                    out.push(dir_key(d));
                }
            }
        }
        out
    }
}

fn dir_of(b: Button) -> Option<usize> {
    match b {
        Button::DPadLeft => Some(0),
        Button::DPadRight => Some(1),
        Button::DPadUp => Some(2),
        Button::DPadDown => Some(3),
        _ => None,
    }
}

fn dir_key(d: usize) -> KeyEvent {
    key([KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down][d])
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

/// South fires or chooses, East goes back or sells, the shoulders pick
/// weapons, the triggers set power in big steps, Start moves on.
fn button_key(b: Button, ctx: Context) -> Option<KeyEvent> {
    use KeyCode::*;
    Some(key(match (ctx, b) {
        (Context::Play, Button::South) => Char(' '),
        (Context::Play, Button::East) => Char('s'),
        (Context::Play, Button::West) => Char('b'),
        (Context::Play, Button::North) => Char('c'),
        (Context::Play, Button::LeftTrigger) => BackTab,
        (Context::Play, Button::RightTrigger) => Tab,
        (Context::Play, Button::LeftTrigger2) => PageDown,
        (Context::Play, Button::RightTrigger2) => PageUp,
        (Context::Play, Button::Start) => Enter,
        (Context::Play, Button::Select) => Char('q'),
        (Context::Shop, Button::South) => Enter,
        (Context::Shop, Button::East) => Left,
        (Context::Shop, Button::West | Button::North) => Tab,
        (Context::Shop, Button::Start) => Char(' '),
        (Context::Shop, Button::Select) => Char('q'),
        (Context::Menu, Button::South | Button::Start) => Enter,
        (Context::Menu, Button::East | Button::Select) => Esc,
        _ => return None,
    }))
}
