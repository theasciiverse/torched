//! Drawing: the playfield as one Braille canvas scaled from world units,
//! the HUD and score boxes as ordinary widgets.

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Widget},
};

use crate::land;
use crate::economy::ShieldKind;
use crate::kit::Aisle;
use crate::physics::Walls;
use crate::sim::{BlastKind, Game, Phase, TANK_HEIGHT, TANK_RADIUS, TURRET_RADIUS};

pub const AMBER: Color = Color::Rgb(255, 176, 0);
const AMBER_DIM: Color = Color::Rgb(150, 100, 20);
const SKY: Color = Color::Rgb(8, 10, 22);
const TANK_COLOURS: [Color; 4] = [
    Color::Rgb(255, 95, 85),
    Color::Rgb(90, 200, 255),
    Color::Rgb(130, 235, 110),
    Color::Rgb(235, 130, 255),
];
const TRAIL_OLD: Color = Color::Rgb(90, 90, 110);
const BAR: Color = Color::Rgb(25, 18, 5);

/// A colour at half strength, for things left behind.
fn dim(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(r / 2 + 20, g / 2 + 20, b / 2 + 30),
        c => c,
    }
}

pub fn tank_colour(i: usize) -> Color {
    TANK_COLOURS[i % TANK_COLOURS.len()]
}

pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 12;

pub fn draw(f: &mut Frame, g: &Game) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(SKY)), area);
    if area.width < MIN_W || area.height < MIN_H {
        let msg = Paragraph::new(format!("Torched needs at least {MIN_W}x{MIN_H}"))
            .style(Style::new().fg(AMBER));
        f.render_widget(msg, area);
        return;
    }
    let [hud, field, foot] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Hud(g), hud);
    if g.phase != Phase::Shop {
        f.render_widget(Field(g), field);
    }
    f.render_widget(Footer(g), foot);
    match g.phase {
        Phase::RoundOver | Phase::GameOver => scores(f, g, field),
        Phase::Shop => shop(f, g, field),
        _ => {}
    }
}

/// Money as "$12,345".
pub fn cash(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("{}${out}", if n < 0 { "-" } else { "" })
}

fn shield_colour(kind: Option<ShieldKind>) -> Color {
    match kind {
        Some(ShieldKind::Force) => Color::Rgb(255, 230, 130),
        Some(ShieldKind::Magnetic) => Color::Rgb(175, 150, 255),
        _ => Color::Rgb(200, 205, 220),
    }
}

struct Hud<'a>(&'a Game);

impl Widget for Hud<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let t = g.current();
        let label = Style::new().fg(AMBER_DIM);
        let value = Style::new().fg(AMBER).add_modifier(Modifier::BOLD);
        let bar = Style::new().bg(BAR);
        let [top, bottom] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        if matches!(g.phase, Phase::RoundOver | Phase::GameOver) {
            let text = if g.phase == Phase::GameOver { " Game over" } else { " Round over" };
            Line::styled(text, value).style(bar).render(top, buf);
        } else if g.phase == Phase::Shop {
            let p = &g.tanks[g.shop.player];
            Line::from(vec![
                Span::styled(format!(" {} ", p.name), Style::new().fg(Color::Black).bg(tank_colour(p.colour)).add_modifier(Modifier::BOLD)),
                Span::styled("  shopping  ", label),
                Span::styled(cash(p.money), value),
            ])
            .style(bar)
            .render(top, buf);
        } else {
            let weapon = g.weapon(t.weapon);
            let count = match g.weapon_count(g.turn, t.weapon) {
                Some(n) => format!(" ×{n}"),
                None => String::new(),
            };
            Line::from(vec![
                Span::styled(format!(" {} ", t.name), Style::new().fg(Color::Black).bg(tank_colour(t.colour)).add_modifier(Modifier::BOLD)),
                Span::styled("  Angle ", label),
                Span::styled(format!("{:>3}°", t.angle), value),
                Span::styled("  Power ", label),
                Span::styled(format!("{:>4}", t.power), value),
                Span::styled("  Life ", label),
                Span::styled(format!("{:>4}", t.life.max(0)), value),
                Span::styled("  ", label),
                Span::styled(weapon.name.clone(), Style::new().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(count, label),
            ])
            .style(bar)
            .render(top, buf);
        }
        // Wind as Xscorch shows it, scaled so the default maximum is 50.
        let w = (g.wind * 1000.0).round() as i32;
        let arrow = match w.signum() {
            -1 => "◀ ",
            1 => "▶ ",
            _ => "",
        };
        let mut spans = vec![
            Span::styled(" Wind ", label),
            Span::styled(format!("{arrow}{}", w.abs()), value),
            Span::styled("  Walls ", label),
            Span::styled(g.walls.name(), value),
        ];
        if g.cfg.viscosity > 0.0 {
            spans.push(Span::styled("  Air ", label));
            spans.push(Span::styled(format!("{:.3}", g.cfg.viscosity), value));
        }
        // What the tank carries that matters now.
        if matches!(g.phase, Phase::Aim | Phase::Action | Phase::Settle) {
            let i = g.turn;
            if let Some(sh) = t.shield {
                let a = &g.accessories.items[sh.acc];
                spans.push(Span::styled("  ", label));
                spans.push(Span::styled(format!("{} {}", a.name, sh.life), Style::new().fg(shield_colour(a.shield))));
            }
            let shields = g.count(i, |a| a.shield.is_some());
            if shields > 0 {
                spans.push(Span::styled(format!("  Shields {shields}"), label));
            }
            let fuel = g.fuel(i);
            if fuel > 0 {
                spans.push(Span::styled(format!("  Fuel {fuel}"), label));
            }
            let batt = g.count(i, |a| a.battery);
            if batt > 0 {
                spans.push(Span::styled(format!("  Batt {batt}"), label));
            }
            if g.has(i, |a| a.contact_trigger) {
                let on = if t.triggers { "on" } else { "off" };
                spans.push(Span::styled(format!("  Trig {on}"), label));
            }
        }
        let right = if g.phase == Phase::Shop {
            format!("Round {}/{} ", g.round + 1, g.cfg.rounds)
        } else {
            let money = if g.cfg.free { String::new() } else { format!("{}  ", cash(t.money)) };
            format!("{money}Round {}/{}  {} ", g.round, g.cfg.rounds, g.generator.name())
        };
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let pad = (area.width as usize).saturating_sub(used + right.chars().count());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right, label));
        Line::from(spans).style(bar).render(bottom, buf);
    }
}

struct Footer<'a>(&'a Game);

impl Widget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let aim_help = || {
            let i = g.turn;
            let mut h = String::from("←→↑↓ aim  Tab weapon");
            for (want, text) in [
                (g.has(i, |a| a.shield.is_some()), "  s shield"),
                (g.has(i, |a| a.battery), "  b battery"),
                (g.fuel(i) > 0, "  a d drive"),
                (g.has(i, |a| a.contact_trigger), "  c trigger"),
                (g.has(i, |a| a.teleporter), "  x teleport"),
            ] {
                if want {
                    h.push_str(text);
                }
            }
            h.push_str("  Space fire");
            h
        };
        let help = match g.phase {
            Phase::Aim => aim_help(),
            Phase::Shop => "↑↓ choose  Enter buy  ← sell  Tab aisle  Space done".into(),
            Phase::RoundOver => "Enter next round  q quit".into(),
            Phase::GameOver => "Enter new game  q quit".into(),
            _ => String::new(),
        };
        let help = help.as_str();
        let msg = Span::styled(format!(" {}", g.message), Style::new().fg(AMBER));
        let used = g.message.chars().count() + 1;
        // The message wins when both don't fit.
        let help = if used + help.chars().count() + 2 > area.width as usize { "" } else { help };
        let pad = (area.width as usize).saturating_sub(used + help.chars().count() + 1);
        Line::from(vec![msg, Span::raw(" ".repeat(pad)), Span::styled(help, Style::new().fg(AMBER_DIM))])
            .style(Style::new().bg(BAR))
            .render(area, buf);
    }
}

/// A Braille canvas: 2x4 dots per cell, one colour per cell, the colour
/// of the most important thing drawn in it.
struct Canvas {
    w: i32,
    h: i32,
    cols: u16,
    bits: Vec<u8>,
    colour: Vec<Color>,
    prio: Vec<u8>,
}

impl Canvas {
    fn new(cols: u16, rows: u16) -> Canvas {
        let n = cols as usize * rows as usize;
        Canvas { w: cols as i32 * 2, h: rows as i32 * 4, cols, bits: vec![0; n], colour: vec![Color::Reset; n], prio: vec![0; n] }
    }

    fn dot(&mut self, x: i32, y: i32, colour: Color, prio: u8) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        const BIT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let cell = (y / 4) as usize * self.cols as usize + (x / 2) as usize;
        self.bits[cell] |= BIT[(x % 2) as usize][(y % 4) as usize];
        if prio >= self.prio[cell] {
            self.prio[cell] = prio;
            self.colour[cell] = colour;
        }
    }

    fn line(&mut self, (x0, y0): (f64, f64), (x1, y1): (f64, f64), colour: Color, prio: u8) {
        let n = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as i32;
        for i in 0..=n {
            let f = i as f64 / n as f64;
            self.dot((x0 + (x1 - x0) * f).round() as i32, (y0 + (y1 - y0) * f).round() as i32, colour, prio);
        }
    }
}

struct Field<'a>(&'a Game);

impl Widget for Field<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let mut c = Canvas::new(area.width, area.height);
        // Dots per world unit; stretched to fill, so every window plays
        // the same field.
        let sx = c.w as f64 / land::WIDTH as f64;
        let sy = c.h as f64 / land::HEIGHT as f64;
        let to_dot = |x: f64, y: f64| (x * sx, (land::HEIGHT as f64 - y) * sy);

        // Ground, sampled at each dot's centre, shaded by height.
        for dy in 0..c.h {
            let wy = land::HEIGHT as f64 - (dy as f64 + 0.5) / sy;
            let shade = (wy / land::MAX_HEIGHT).clamp(0.0, 1.0);
            let colour = Color::Rgb(
                (70.0 + 110.0 * shade) as u8,
                (55.0 + 85.0 * shade) as u8,
                (30.0 + 30.0 * shade) as u8,
            );
            for dx in 0..c.w {
                let wx = (dx as f64 + 0.5) / sx;
                if g.land.get(wx as i32, wy as i32) {
                    c.dot(dx, dy, colour, 1);
                }
            }
        }

        // The aiming player's last shot, faint.
        if g.phase == Phase::Aim {
            let trail = &g.current().last_trail;
            for (i, p) in trail.iter().enumerate().step_by(2) {
                let (x, y) = to_dot(p.0, p.1);
                if i % 4 == 0 {
                    c.dot(x.round() as i32, y.round() as i32, TRAIL_OLD, 2);
                }
            }
        }

        // Tanks: a dome and a barrel, never smaller than a few dots.
        let hw = (TANK_RADIUS as f64 * sx).round().max(2.0) as i32;
        let hh = (TANK_HEIGHT as f64 * sy).round().max(2.0) as i32;
        let barrel = (TURRET_RADIUS * (sx + sy) / 2.0).max(3.0);
        for (i, t) in g.tanks.iter().enumerate() {
            if !t.alive {
                continue;
            }
            let colour = tank_colour(t.colour);
            let (bx, by) = to_dot(t.x as f64, t.y as f64);
            let (bx, by) = (bx.round() as i32, by.round() as i32);
            for r in 0..hh {
                // Full-width tracks, then a hull tapering to the turret.
                let w = hw - (r * hw + hh) / (hh + 1);
                for x in bx - w..=bx + w {
                    c.dot(x, by - 1 - r, colour, 4);
                }
            }
            let a = (t.angle as f64).to_radians();
            let (vx, vy) = (a.cos() * sx, -a.sin() * sy);
            let len = (vx * vx + vy * vy).sqrt();
            let pivot = (bx as f64, (by - hh) as f64);
            let tip = (pivot.0 + vx / len * barrel, pivot.1 + vy / len * barrel);
            c.line(pivot, tip, colour, 4);
            // Mark whose turn it is with a pip above the tank.
            if i == g.turn && g.phase == Phase::Aim {
                c.dot(bx, by - hh - barrel.ceil() as i32 - 2, AMBER, 5);
            }
        }

        // Shields: a dome over each tank that has one up, always a little
        // bigger than the tank's own dots so it shows at any size.
        for t in g.tanks.iter().filter(|t| t.alive) {
            let Some(sh) = t.shield else { continue };
            let a = &g.accessories.items[sh.acc];
            let colour = shield_colour(a.shield);
            let (bx, by) = to_dot(t.x as f64, t.y as f64);
            let r = (TANK_RADIUS + 1) as f64;
            let (rx, ry) = ((r * sx).max(hw as f64 + 1.5), (r * sy).max(hh as f64 + 1.5));
            let steps = 48;
            for k in 0..=steps {
                let ang = std::f64::consts::PI * k as f64 / steps as f64;
                c.dot((bx + rx * ang.cos()).round() as i32, (by - ry * ang.sin()).round() as i32, colour, 3);
            }
        }

        // Smoke tracers' trails stay for the round.
        for sm in &g.smoke {
            let colour = dim(tank_colour(g.tanks[sm.owner].colour));
            for w in sm.points.windows(2) {
                if (w[1].0 - w[0].0).abs() < land::WIDTH as f64 / 2.0 {
                    c.line(to_dot(w[0].0, w[0].1), to_dot(w[1].0, w[1].1), colour, 2);
                }
            }
        }

        // Shells and their trails.
        for sh in &g.shells {
            if sh.delay > 0 {
                continue;
            }
            let colour = tank_colour(g.tanks[sh.owner].colour);
            for w in sh.trail.windows(2).rev().take(40) {
                // A wrap across the field isn't a line across the screen.
                if (w[1].0 - w[0].0).abs() < land::WIDTH as f64 / 2.0 {
                    c.line(to_dot(w[0].0, w[0].1), to_dot(w[1].0, w[1].1), colour, 3);
                }
            }
            let (x, y) = to_dot(sh.x, sh.y);
            let head = if sh.rolling() {
                AMBER
            } else if sh.digging() {
                Color::Rgb(255, 140, 60)
            } else {
                Color::White
            };
            c.dot(x.round() as i32, y.round() as i32, head, 6);
        }

        for b in &g.blasts {
            // A spider leg's arc shows while the leg is on its way.
            if !b.arc.is_empty() && (b.delay > 0 || b.age < b.grow) {
                for w in b.arc.windows(2) {
                    c.line(to_dot(w[0].0, w[0].1), to_dot(w[1].0, w[1].1), Color::Rgb(255, 200, 120), 3);
                }
            }
            if !b.active() {
                continue;
            }
            match b.kind {
                BlastKind::Napalm | BlastKind::LiquidDirt => {
                    let fade = b.fade();
                    for (k, &(x, y)) in b.wet().iter().enumerate() {
                        let colour = if b.kind == BlastKind::LiquidDirt {
                            Color::Rgb(150, 110, 60)
                        } else {
                            // Flames flicker cell by cell, dying down.
                            let f = ((k as u32).wrapping_mul(2654435761).wrapping_add(b.age * 40503) >> 24) as f64 / 255.0;
                            let heat = (f - fade * 0.8).clamp(0.0, 1.0);
                            Color::Rgb(255, (40.0 + 200.0 * heat) as u8, (10.0 + 60.0 * heat * heat) as u8)
                        };
                        let (dx, dy) = to_dot(x as f64 + 0.5, y as f64 + 0.5);
                        c.dot(dx as i32, dy as i32, colour, 7);
                    }
                }
                kind => {
                    let r = b.shown_radius();
                    let (cx, cy) = to_dot(b.x, b.y);
                    let (rx, ry) = (r * sx, r * sy);
                    let fade = b.fade();
                    for dy in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
                        for dx in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                            let (nx, ny) = ((dx as f64 - cx) / rx.max(0.5), (dy as f64 - cy) / ry.max(0.5));
                            let d = nx * nx + ny * ny;
                            if d > 1.0 {
                                continue;
                            }
                            if let Some((dir, width)) = b.wedge {
                                // Screen y is down; world angles are up.
                                let off = ((-ny).atan2(nx) - dir + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                                    - std::f64::consts::PI;
                                if off.abs() > width / 2.0 {
                                    continue;
                                }
                            }
                            let heat = (1.0 - d.sqrt() * 0.7 - fade * 0.6).clamp(0.0, 1.0);
                            let colour = match kind {
                                // Nukes burn white and violet.
                                BlastKind::Plasma => Color::Rgb((150.0 + 105.0 * heat) as u8, (90.0 + 165.0 * heat) as u8, 255),
                                // Riots are a puff of dust, dirt a clod.
                                BlastKind::Riot => Color::Rgb((120.0 + 100.0 * heat) as u8, (110.0 + 90.0 * heat) as u8, (90.0 + 60.0 * heat) as u8),
                                BlastKind::Dirt => Color::Rgb(160, 120, 65),
                                _ => Color::Rgb(255, (60.0 + 190.0 * heat) as u8, (20.0 + 120.0 * heat * heat) as u8),
                            };
                            // Riot dust is thin: every other dot.
                            if kind == BlastKind::Riot && (dx + dy) % 2 != 0 {
                                continue;
                            }
                            c.dot(dx, dy, colour, 7);
                        }
                    }
                }
            }
        }

        // Walls that stop or bounce a shot are drawn down the sides.
        let wall = match g.walls {
            Walls::Concrete => Some(Color::Rgb(140, 140, 150)),
            Walls::Padded => Some(Color::Rgb(90, 140, 200)),
            Walls::Rubber => Some(Color::Rgb(220, 80, 160)),
            Walls::Spring => Some(Color::Rgb(120, 230, 180)),
            _ => None,
        };
        if let Some(colour) = wall {
            for dy in (0..c.h).step_by(2) {
                c.dot(0, dy, colour, 2);
                c.dot(c.w - 1, dy, colour, 2);
            }
        }

        // Stars, last and only into empty cells, so they never take a
        // cell's colour from the ground or a tank.
        for st in &g.stars {
            let (x, y) = to_dot(st.x, st.y);
            let (x, y) = (x as i32, y as i32);
            if x < 0 || y < 0 || x >= c.w || y >= c.h {
                continue;
            }
            let cell = (y / 4) as usize * c.cols as usize + (x / 2) as usize;
            if c.bits[cell] != 0 {
                continue;
            }
            let wave = if st.twinkle > 0.0 { 0.55 + 0.45 * (g.clock as f64 * st.twinkle + st.phase).sin() } else { 1.0 };
            let v = (st.bright * wave).clamp(0.1, 1.0);
            let (r, gg, b) = match st.tint {
                1 => (255.0, 210.0, 150.0),
                2 => (160.0, 190.0, 255.0),
                _ => (220.0, 225.0, 240.0),
            };
            let k = 0.2 + 0.8 * v;
            c.dot(x, y, Color::Rgb((r * k) as u8, (gg * k) as u8, (b * k) as u8), 0);
        }

        for row in 0..area.height {
            for col in 0..area.width {
                let i = row as usize * c.cols as usize + col as usize;
                if c.bits[i] == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + c.bits[i] as u32).unwrap_or(' ');
                if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                    cell.set_char(ch).set_fg(c.colour[i]);
                }
            }
        }
    }
}

/// The shop: one player at a time, weapons or accessories.
fn shop(f: &mut Frame, g: &Game, field: Rect) {
    let p = &g.tanks[g.shop.player];
    let rows = g.shop_rows();
    let w = 72.min(field.width);
    let h = field.height;
    let r = Rect { x: field.x + (field.width - w) / 2, y: field.y, width: w, height: h };
    let label = Style::new().fg(AMBER_DIM);
    let tab = |a: Aisle, text: &'static str| {
        if g.shop.aisle == a {
            Span::styled(format!(" {text} "), Style::new().fg(Color::Black).bg(AMBER).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(format!(" {text} "), label)
        }
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", p.name), Style::new().fg(tank_colour(p.colour)).add_modifier(Modifier::BOLD)),
            Span::styled(cash(p.money), Style::new().fg(AMBER).add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            tab(Aisle::Weapons, "Weapons"),
            Span::raw(" "),
            tab(Aisle::Accessories, "Accessories"),
        ]),
        Line::styled(format!("{:<24}{:>8}{:>12}{:>6}", "", "bundle", "price", "have"), label),
    ];
    // Rows that fit, scrolled to keep the cursor in view; three lines
    // at the bottom for the description and a gap.
    let room = (h as usize).saturating_sub(2 + lines.len() + 3).max(1);
    let first = g.shop.cursor.saturating_sub(room.saturating_sub(1)).min(rows.len().saturating_sub(room));
    for (k, row) in rows.iter().enumerate().skip(first).take(room) {
        let selected = k == g.shop.cursor;
        let mut style = if row.affordable { Style::new().fg(AMBER) } else { Style::new().fg(Color::Rgb(110, 85, 40)) };
        if selected {
            style = style.bg(Color::Rgb(70, 50, 10)).add_modifier(Modifier::BOLD);
        }
        let have = if row.have > 0 { row.have.to_string() } else { String::new() };
        let mark = if selected { "▶ " } else { "  " };
        lines.push(Line::styled(format!("{mark}{:<22}{:>8}{:>12}{:>6}", row.name, row.bundle, cash(row.price as i64), have), style));
    }
    while lines.len() < 2 + room {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    if let Some(row) = rows.get(g.shop.cursor) {
        let note = if row.buy.0 == 0 {
            "Can't carry more.".to_string()
        } else if row.buy.0 < row.bundle {
            format!("{} more for {} (a broken bundle costs extra).", row.buy.0, cash(row.buy.1))
        } else {
            String::new()
        };
        lines.push(Line::styled(format!("{} {note}", row.description), Style::new().fg(Color::Rgb(220, 210, 190))));
    }
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(format!(" Shop · round {} of {} ", g.round + 1, g.cfg.rounds))
                .title_bottom(Line::styled(format!(" {} scoring: {} ", g.scoring.name, g.scoring.description), label))
                .border_style(Style::new().fg(AMBER))
                .style(Style::new().bg(Color::Rgb(20, 14, 4))),
        ),
        r,
    );
}

fn scores(f: &mut Frame, g: &Game, field: Rect) {
    let title = if g.phase == Phase::GameOver { " Final standings " } else { " Scores " };
    let mut lines = vec![Line::styled(g.message.clone(), Style::new().fg(AMBER).add_modifier(Modifier::BOLD)), Line::raw("")];
    for (rank, &i) in g.standings().iter().enumerate() {
        let t = &g.tanks[i];
        lines.push(Line::from(vec![
            Span::styled(format!("{}. ", rank + 1), Style::new().fg(AMBER_DIM)),
            Span::styled(format!("{:<10}", t.name), Style::new().fg(tank_colour(t.colour)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:>3} wins {:>3} kills ", t.wins, t.kills), Style::new().fg(AMBER)),
            Span::styled(if g.cfg.free { String::new() } else { format!("{:>10}", cash(t.money)) }, Style::new().fg(AMBER_DIM)),
        ]));
    }
    lines.push(Line::raw(""));
    let hint = if g.phase == Phase::GameOver { "Enter: new game" } else { "Enter: next round" };
    lines.push(Line::styled(hint, Style::new().fg(AMBER_DIM)));

    let w = 48.min(field.width);
    let h = (lines.len() as u16 + 2).min(field.height);
    let r = Rect { x: field.x + (field.width - w) / 2, y: field.y + (field.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).centered().block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(title)
                .border_style(Style::new().fg(AMBER))
                .style(Style::new().bg(Color::Rgb(20, 14, 4))),
        ),
        r,
    );
}
