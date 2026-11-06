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
use crate::sim::{Game, Phase, TANK_HEIGHT, TANK_RADIUS, TURRET_RADIUS};

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
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Hud(g), hud);
    f.render_widget(Field(g), field);
    f.render_widget(Footer(g), foot);
    match g.phase {
        Phase::RoundOver | Phase::GameOver => scores(f, g, field),
        _ => {}
    }
}

struct Hud<'a>(&'a Game);

impl Widget for Hud<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let t = g.current();
        let label = Style::new().fg(AMBER_DIM);
        let value = Style::new().fg(AMBER).add_modifier(Modifier::BOLD);
        if matches!(g.phase, Phase::RoundOver | Phase::GameOver) {
            let text = if g.phase == Phase::GameOver { " Game over" } else { " Round over" };
            Line::styled(text, value).style(Style::new().bg(Color::Rgb(25, 18, 5))).render(area, buf);
            return;
        }
        let mut spans = vec![
            Span::styled(format!(" {} ", t.name), Style::new().fg(Color::Black).bg(tank_colour(t.colour)).add_modifier(Modifier::BOLD)),
            Span::styled("  Angle ", label),
            Span::styled(format!("{:>3}°", t.angle), value),
            Span::styled("  Power ", label),
            Span::styled(format!("{:>4}", t.power), value),
            Span::styled("  Life ", label),
            Span::styled(format!("{:>4}", t.life.max(0)), value),
            Span::styled("  Wind ", label),
            Span::styled("0", value),
        ];
        let right = format!("Round {}/{}  {} ", g.round, g.cfg.rounds, g.generator.name());
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let pad = (area.width as usize).saturating_sub(used + right.chars().count());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right, label));
        Line::from(spans).style(Style::new().bg(Color::Rgb(25, 18, 5))).render(area, buf);
    }
}

struct Footer<'a>(&'a Game);

impl Widget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let help = match g.phase {
            Phase::Aim => "←→ angle  ↑↓ power  Shift or PgUp/PgDn: big steps  Space fire  q quit",
            Phase::RoundOver => "Enter next round  q quit",
            Phase::GameOver => "Enter new game  q quit",
            _ => "",
        };
        let msg = Span::styled(format!(" {}", g.message), Style::new().fg(AMBER));
        let used = g.message.chars().count() + 1;
        // The message wins when both don't fit.
        let help = if used + help.chars().count() + 2 > area.width as usize { "" } else { help };
        let pad = (area.width as usize).saturating_sub(used + help.chars().count() + 1);
        Line::from(vec![msg, Span::raw(" ".repeat(pad)), Span::styled(help, Style::new().fg(AMBER_DIM))])
            .style(Style::new().bg(Color::Rgb(25, 18, 5)))
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

        // The shot and its trail.
        if let Some(s) = &g.shot {
            let colour = tank_colour(g.tanks[s.owner].colour);
            for w in s.trail.windows(2).rev().take(40) {
                c.line(to_dot(w[0].0, w[0].1), to_dot(w[1].0, w[1].1), colour, 3);
            }
            let (x, y) = to_dot(s.x, s.y);
            c.dot(x.round() as i32, y.round() as i32, Color::White, 6);
        }

        // The blast: a ball of fire cooling from yellow to red.
        if let Some(b) = &g.blast {
            let r = b.shown_radius();
            let (cx, cy) = to_dot(b.x, b.y);
            let (rx, ry) = (r * sx, r * sy);
            let fade = b.fade();
            for dy in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
                for dx in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                    let (nx, ny) = ((dx as f64 - cx) / rx.max(0.5), (dy as f64 - cy) / ry.max(0.5));
                    let d = nx * nx + ny * ny;
                    if d <= 1.0 {
                        let heat = (1.0 - d.sqrt() * 0.7 - fade * 0.6).clamp(0.0, 1.0);
                        let colour = Color::Rgb(255, (60.0 + 190.0 * heat) as u8, (20.0 + 120.0 * heat * heat) as u8);
                        c.dot(dx, dy, colour, 7);
                    }
                }
            }
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

fn scores(f: &mut Frame, g: &Game, field: Rect) {
    let title = if g.phase == Phase::GameOver { " Final standings " } else { " Scores " };
    let mut lines = vec![Line::styled(g.message.clone(), Style::new().fg(AMBER).add_modifier(Modifier::BOLD)), Line::raw("")];
    for (rank, &i) in g.standings().iter().enumerate() {
        let t = &g.tanks[i];
        lines.push(Line::from(vec![
            Span::styled(format!("{}. ", rank + 1), Style::new().fg(AMBER_DIM)),
            Span::styled(format!("{:<10}", t.name), Style::new().fg(tank_colour(t.colour)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:>3} wins {:>3} kills", t.wins, t.kills), Style::new().fg(AMBER)),
        ]));
    }
    lines.push(Line::raw(""));
    let hint = if g.phase == Phase::GameOver { "Enter: new game" } else { "Enter: next round" };
    lines.push(Line::styled(hint, Style::new().fg(AMBER_DIM)));

    let w = 36.min(field.width);
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
