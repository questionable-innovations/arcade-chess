//! Board lighting: composes a desired frame, diffs it against the last one
//! sent, and emits the minimum number of device commands.
//!
//! # What the hardware actually allows
//!
//! `lighting.set` maps to UART `SET_SQUARES`, and the AVR does
//! `override_mask_ = mask` (`firmware-atmega/src/lighting.cpp`) — *replace*, not
//! merge — with **one colour**. So each quadrant can display exactly one
//! override colour at a time, and "missing squares amber, extra squares red" is
//! not implementable on stock firmware: squares 12 and 13 are both in node 1.
//!
//! That constraint is what the Basic tier is built around, and it still
//! delivers the setup experience, which is the part that matters:
//!
//! > Override mask = *only the target squares that are still empty*, in amber.
//! > When a piece lands, drop that square from the mask so it falls through to
//! > the sensor colour. **Amber = still needed. White = piece detected. No amber
//! > left = board built.**
//!
//! Self-correcting, zero firmware change, and honest: a dead sensor never turns
//! white, which is what the Start button is for.
//!
//! # Bus budget
//!
//! One 38400-baud multidrop line, one outstanding transaction, an eight-deep ESP
//! queue (`firmware-esp/src/bus_manager.h`), and a 25 fps render window the
//! quadrants shift their strips inside. A full-board update is up to four
//! enqueues. Flooding the bus looks exactly like "the board froze", so: paint on
//! state change only, plus a 1 Hz re-assert, hard cap five frames a second.
//! Animate on screen, snap the hardware.

use serde_json::{json, Value};

use super::config::{BarSlot, Palette};
use super::observe::{NODES, SQUARES};

/// Hard ceiling on command batches per second, whatever the game asks for.
const MAX_FRAMES_PER_SECOND: u32 = 5;
/// A stray probe from the debug console self-heals within this long.
const REASSERT_MS: u64 = 1000;
/// Half-bar writes deferred past this many per batch, so lighting and bars
/// together never exceed the eight-deep bus queue.
const MAX_BAR_WRITES_PER_FRAME: usize = 4;
pub const BAR_PIXELS: usize = 16;
pub const SIDES: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(self) -> String {
        format!("{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// What one square is trying to say. Ordering is priority: the Basic tier can
/// only render one class at a time, and the frame builder names which.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Paint {
    /// Something is wrong here — an extra piece, an illegal settle.
    Alert,
    /// A piece is wanted on this square and is not there yet.
    Needed,
    /// The move that just happened, or a candidate awaiting a tap.
    Focus,
    /// Winner colour during the finish sweep.
    Sweep,
}

impl Rgb {
    pub fn from_u32(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
}

impl Paint {
    pub fn rgb(self, palette: &Palette) -> Rgb {
        Rgb::from_u32(match self {
            Paint::Alert => palette.alert,
            Paint::Needed => palette.needed,
            Paint::Focus => palette.focus,
            Paint::Sweep => palette.sweep,
        })
    }
}

/// The board we would like to be looking at, in game coordinates.
#[derive(Clone)]
pub struct Frame {
    pub squares: [Option<Paint>; SQUARES],
    /// Which paint class a Basic-tier quadrant renders. Everything else falls
    /// through to the sensor colour, which is the whole trick.
    pub basic: Option<Paint>,
    pub bars: [[Rgb; BAR_PIXELS]; SIDES],
    pub bars_wanted: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            squares: [None; SQUARES],
            basic: None,
            bars: [[Rgb(0, 0, 0); BAR_PIXELS]; SIDES],
            bars_wanted: false,
        }
    }
}

impl Frame {
    pub fn set(&mut self, square: u8, paint: Paint) {
        if (square as usize) < SQUARES {
            self.squares[square as usize] = Some(paint);
        }
    }

    /// Fills a tug-of-war bar: `fill` pixels in white's colour, the rest in
    /// black's. Clamped short of the ends so the bar never loses its
    /// orientation, even at mate.
    pub fn eval_bar(&mut self, side: usize, win_prob_white: f64, palette: &Palette) {
        let fill = (win_prob_white * BAR_PIXELS as f64).round() as i64;
        let fill = fill.clamp(1, BAR_PIXELS as i64 - 1) as usize;
        let (white, black) = (
            Rgb::from_u32(palette.bar_white),
            Rgb::from_u32(palette.bar_black),
        );
        for pixel in 0..BAR_PIXELS {
            self.bars[side][pixel] = if pixel < fill { white } else { black };
        }
    }

    pub fn turn_bar(&mut self, side: usize, white_to_move: bool, palette: &Palette) {
        let colour = if white_to_move {
            Rgb::from_u32(palette.bar_white)
        } else {
            Rgb::from_u32(palette.bar_black)
        };
        self.bars[side] = [colour; BAR_PIXELS];
    }
}

/// Where one half of one edge bar physically lives. Which half-bar lands on
/// which side depends on a hardware bodge still listed under bring-up
/// requirements, so this is unknowable until game time and must be editable
/// live — not an env var, which would mean a redeploy at the venue.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct BarHalf {
    pub node: u8,
    /// `"a"` (PE0) or `"b"` (PE1).
    pub strip: char,
    pub reversed: bool,
}

#[derive(Debug)]
pub struct Command {
    pub name: &'static str,
    pub args: Value,
}

pub struct Painter {
    /// Squares currently overridden on the device, in device coordinates, with
    /// the colour they were sent as.
    live: [Option<Rgb>; SQUARES],
    live_bars: [[Rgb; BAR_PIXELS]; SIDES],
    bars_written: bool,
    last_assert_ms: u64,
    window_start_ms: u64,
    frames_in_window: u32,
    /// Sensor colours neutralised for this game, so "a piece is here" is one
    /// consistent colour instead of the random per-piece polarity hues.
    pub colours_neutralised: bool,
    pub bar_map: [[BarHalf; 2]; SIDES],
    /// Cleared by the first `node_error code=2` from a bar write.
    pub bars_supported: bool,
    pub palette: Palette,
}

/// A guessed default that the assignment tool in the admin rail exists to
/// correct. Two minutes at the venue turns an unknown into a form to fill in.
fn default_bar_map() -> [[BarHalf; 2]; SIDES] {
    let half = |node, strip| BarHalf {
        node,
        strip,
        reversed: false,
    };
    [
        [half(0, 'a'), half(1, 'a')],
        [half(1, 'b'), half(3, 'b')],
        [half(3, 'a'), half(2, 'a')],
        [half(2, 'b'), half(0, 'b')],
    ]
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub fn new() -> Painter {
        Painter {
            live: [None; SQUARES],
            live_bars: [[Rgb(0, 0, 0); BAR_PIXELS]; SIDES],
            bars_written: false,
            last_assert_ms: 0,
            window_start_ms: 0,
            frames_in_window: 0,
            colours_neutralised: false,
            bar_map: default_bar_map(),
            bars_supported: true,
            palette: Palette::default(),
        }
    }

    /// A quadrant that dropped and came back has rebooted: it has forgotten its
    /// override mask, its bar contents and its colour keys, and the AVR's own
    /// `bar_written_` flag went with them.
    ///
    /// Everything this painter believes about that node is therefore stale, and
    /// nothing on the wire will ever tell it so — `lighting.set` only addresses
    /// the nodes its square list names, and the 1 Hz re-assert covers squares
    /// only. So the belief is torn up here and rebuilt from the next frame.
    pub fn node_rejoined(&mut self, node: usize) {
        if node >= NODES {
            return;
        }
        for sq in 0..SQUARES {
            if node_of(sq as u8) == node {
                self.live[sq] = None;
            }
        }
        self.bars_written = false;
        self.colours_neutralised = false;
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// The bar map as the venue profile stores it.
    pub fn bar_map_slots(&self) -> [[BarSlot; 2]; SIDES] {
        std::array::from_fn(|side| {
            std::array::from_fn(|half| {
                let h = self.bar_map[side][half];
                BarSlot {
                    node: h.node,
                    strip: h.strip,
                    reversed: h.reversed,
                }
            })
        })
    }

    pub fn set_bar_map_slots(&mut self, slots: &[[BarSlot; 2]; SIDES]) {
        for (side, halves) in slots.iter().enumerate() {
            for (half, slot) in halves.iter().enumerate() {
                self.bar_map[side][half] = BarHalf {
                    node: slot.node,
                    strip: if slot.strip == 'b' { 'b' } else { 'a' },
                    reversed: slot.reversed,
                };
            }
        }
        self.bars_written = false;
    }

    /// An unknown UART message type answers with error code 2, surfaced as
    /// `command.result status=rejected reason=node_error data={node, code:2}`.
    /// That one reply is the whole capability-discovery mechanism: no version
    /// parsing, and it works with mixed firmware across the four quadrants.
    pub fn note_node_error(&mut self, node: usize, code: u64, was_bar_write: bool) -> bool {
        if code != 2 || node >= NODES || !was_bar_write || !self.bars_supported {
            return false;
        }
        self.bars_supported = false;
        self.live_bars = [[Rgb(0, 0, 0); BAR_PIXELS]; SIDES];
        self.bars_written = false;
        true
    }

    /// Forgets what the board is showing without sending anything — used after
    /// an abort, so the next frame is a full repaint rather than a diff against
    /// a board somebody else has since scribbled on.
    pub fn forget(&mut self) {
        self.live = [None; SQUARES];
        self.live_bars = [[Rgb(0, 0, 0); BAR_PIXELS]; SIDES];
        self.bars_written = false;
    }

    /// Turns a desired frame into the commands that would realise it.
    ///
    /// `to_device` maps a game square onto its physical index, so a board
    /// mounted at an angle is corrected in exactly the same place occupancy is.
    pub fn diff(
        &mut self,
        frame: &Frame,
        online: [bool; NODES],
        now_ms: u64,
        to_device: impl Fn(u8) -> u8,
    ) -> Vec<Command> {
        // Rate limiting first: a frame refused here is simply not sent, and the
        // 1 Hz re-assert picks up whatever was missed.
        if now_ms.saturating_sub(self.window_start_ms) >= 1000 {
            self.window_start_ms = now_ms;
            self.frames_in_window = 0;
        }
        let due_for_reassert = now_ms.saturating_sub(self.last_assert_ms) >= REASSERT_MS;

        // Desired device-coordinate colours, dropping everything on a quadrant
        // that cannot show it.
        let mut wanted: [Option<Rgb>; SQUARES] = [None; SQUARES];
        for game_square in 0..SQUARES {
            let Some(paint) = frame.squares[game_square] else {
                continue;
            };
            let device = to_device(game_square as u8);
            let node = node_of(device);
            if !online[node] {
                continue;
            }
            if frame.basic == Some(paint) {
                wanted[device as usize] = Some(paint.rgb(&self.palette));
            }
        }

        let squares_changed = wanted != self.live;
        let bars_changed = frame.bars_wanted
            && self.bars_supported
            && (!self.bars_written || frame.bars != self.live_bars);
        if !squares_changed && !bars_changed && !due_for_reassert {
            return Vec::new();
        }
        if self.frames_in_window >= MAX_FRAMES_PER_SECOND {
            return Vec::new();
        }

        let mut out = Vec::new();
        self.emit_squares(&wanted, online, &mut out);
        if frame.bars_wanted && self.bars_supported {
            self.emit_bars(frame, online, due_for_reassert, &mut out);
        }
        if out.is_empty() {
            return out;
        }

        // Offline quadrants keep whatever they were last told, because they were
        // never sent the clear that would have retired it. Overwriting their
        // entries with `None` loses the fact that the board still owes them one,
        // and the quadrant comes back still showing a four-move-old mask.
        for sq in 0..SQUARES {
            if online[node_of(sq as u8)] {
                self.live[sq] = wanted[sq];
            }
        }
        self.frames_in_window += 1;
        self.last_assert_ms = now_ms;
        out
    }

    fn emit_squares(
        &mut self,
        wanted: &[Option<Rgb>; SQUARES],
        online: [bool; NODES],
        out: &mut Vec<Command>,
    ) {
        // Each quadrant shares one override colour, so the frame's nominated
        // class is the only thing it can show. Squares dropping out of the mask
        // fall through to the sensor colour, which is the "piece landed" half of
        // the language.
        let basic_nodes: Vec<usize> = (0..NODES).filter(|&node| online[node]).collect();
        if basic_nodes.is_empty() {
            return;
        }
        let mut lit: Vec<u8> = Vec::new();
        let mut colour = None;
        for (square, want) in wanted.iter().enumerate() {
            if !basic_nodes.contains(&node_of(square as u8)) {
                continue;
            }
            if let Some(rgb) = want {
                lit.push(square as u8);
                colour = Some(*rgb);
            }
        }
        // A quadrant that had an override and now has none needs an explicit
        // clear: `lighting.set` only ever addresses the nodes its square list
        // names, so a stale mask would otherwise stay lit forever.
        let stale: Vec<u8> = (0..SQUARES)
            .filter(|&sq| {
                basic_nodes.contains(&node_of(sq as u8))
                    && self.live[sq].is_some()
                    && wanted[sq].is_none()
            })
            .map(|sq| sq as u8)
            .collect();
        if !stale.is_empty() {
            out.push(Command {
                name: "lighting.clear",
                args: json!({ "squares": stale }),
            });
        }
        if let (Some(colour), false) = (colour, lit.is_empty()) {
            out.push(Command {
                name: "lighting.set",
                args: json!({
                    "squares": lit,
                    "effect": "solid",
                    "colour": colour.hex(),
                    "duration_ms": 0,
                }),
            });
        }
    }

    /// `reassert` forces a rewrite of halves the painter believes are already
    /// correct.
    ///
    /// Bars need this more than squares do, not less: the AVR drops its
    /// `bar_written_` flag on identify, on the idle breathe and on reboot, and
    /// then reverts the strip to solid white. The turn halves happen to self-heal
    /// because they flip every ply, but the eval bar clamps its fill to 1..15 —
    /// so one of its two halves is *always* constant and could sit stuck white
    /// for a whole game while the screen showed a perfectly correct evaluation.
    fn emit_bars(
        &mut self,
        frame: &Frame,
        online: [bool; NODES],
        reassert: bool,
        out: &mut Vec<Command>,
    ) {
        let mut writes = 0;
        for side in 0..SIDES {
            for half in 0..2 {
                if writes >= MAX_BAR_WRITES_PER_FRAME {
                    // The rest ride the next tick rather than overflowing an
                    // eight-deep bus queue behind the square frame.
                    return;
                }
                let slice: &[Rgb] = &frame.bars[side][half * 8..half * 8 + 8];
                let live: &[Rgb] = &self.live_bars[side][half * 8..half * 8 + 8];
                if self.bars_written && slice == live && !reassert {
                    continue;
                }
                let map = self.bar_map[side][half];
                if map.node as usize >= NODES || !online[map.node as usize] {
                    continue;
                }
                let mut pixels: Vec<String> = slice.iter().map(|c| c.hex()).collect();
                if map.reversed {
                    pixels.reverse();
                }
                out.push(Command {
                    name: "lighting.bar",
                    args: json!({
                        "node": map.node,
                        "strip": map.strip.to_string(),
                        "pixels": pixels,
                    }),
                });
                self.live_bars[side][half * 8..half * 8 + 8].copy_from_slice(slice);
                writes += 1;
            }
        }
        if writes > 0 {
            self.bars_written = true;
        }
    }

    /// Lights one half-bar for the assignment tool. Bypasses the diff entirely:
    /// the whole point is to see which physical strip answers.
    pub fn bars_test(node: u8, strip: char, pixel: Option<usize>) -> Command {
        let pixels: Vec<String> = (0..8)
            .map(|i| match pixel {
                Some(target) if target != i => "000000".to_string(),
                _ => "00d0ff".to_string(),
            })
            .collect();
        Command {
            name: "lighting.bar",
            args: json!({ "node": node, "strip": strip.to_string(), "pixels": pixels }),
        }
    }

    /// Sets both polarity colour keys to one neutral value, so "a piece is
    /// here" reads as one consistent colour instead of green-or-blue at random.
    /// Commits EEPROM (~240 ms), so once per game, not per frame.
    pub fn neutralise_colours(&mut self, online: [bool; NODES]) -> Vec<Command> {
        self.colours_neutralised = true;
        commands_for_colour_keys(online, self.palette.neutralised_rgb565)
    }

    /// Puts the board back the way bring-up expects to find it.
    ///
    /// Order matters. Each config write commits EEPROM (~240 ms) and the bus
    /// queue is eight deep, so sending eight of them *before* the clear is how
    /// the clear gets dropped and the board stays lit through the whole of the
    /// next game's setup. The clear goes first, while the queue is empty.
    pub fn restore_colours(&mut self, online: [bool; NODES]) -> Vec<Command> {
        self.colours_neutralised = false;
        let mut out = vec![Command {
            name: "lighting.clear",
            args: json!({}),
        }];
        // `Lighting::clear` only masks the squares — `bar_written_` stays true on
        // the AVR, so an aborted game's eval bar would otherwise sit lit until
        // something else happened to write that strip.
        if self.bars_supported {
            for side in 0..SIDES {
                for half in 0..2 {
                    let map = self.bar_map[side][half];
                    if map.node as usize >= NODES || !online[map.node as usize] {
                        continue;
                    }
                    out.push(Command {
                        name: "lighting.bar",
                        args: json!({
                            "node": map.node,
                            "strip": map.strip.to_string(),
                            "pixels": vec!["000000".to_string(); 8],
                        }),
                    });
                }
            }
        }
        out.extend(commands_for_colour_keys_split(
            online,
            self.palette.restore_pos_rgb565,
            self.palette.restore_neg_rgb565,
        ));
        self.forget();
        out
    }
}

fn commands_for_colour_keys(online: [bool; NODES], rgb565: u16) -> Vec<Command> {
    commands_for_colour_keys_split(online, rgb565, rgb565)
}

fn commands_for_colour_keys_split(
    online: [bool; NODES],
    positive: u16,
    negative: u16,
) -> Vec<Command> {
    let mut out = Vec::new();
    for (node, up) in online.iter().enumerate() {
        if !up {
            continue;
        }
        // Config keys 7 and 8, `arcade::ConfigKey::kPositiveRgb565` /
        // `kNegativeRgb565` in protocol/include/arcade_protocol/protocol.h.
        out.push(Command {
            name: "node.config.set",
            args: json!({ "node": node, "key": 7, "value": positive }),
        });
        out.push(Command {
            name: "node.config.set",
            args: json!({ "node": node, "key": 8, "value": negative }),
        });
    }
    out
}

fn node_of(device_square: u8) -> usize {
    super::observe::node_of_device_square(device_square)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ONLINE: [bool; NODES] = [true; NODES];

    fn identity(sq: u8) -> u8 {
        sq
    }

    /// A frame with its Basic-tier class already nominated, which every setup
    /// and play frame carries.
    fn frame_for(basic: Paint) -> Frame {
        Frame {
            basic: Some(basic),
            ..Frame::default()
        }
    }


    /// The setup language on stock firmware: amber for what is still wanted,
    /// nothing at all for what has arrived, so the sensor colour shows through.
    #[test]
    fn basic_tier_lights_only_the_nominated_class() {
        let mut painter = Painter::new();
        let mut frame = frame_for(Paint::Needed);
        frame.set(12, Paint::Needed);
        frame.set(13, Paint::Alert);

        let out = painter.diff(&frame, ALL_ONLINE, 1_000, identity);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "lighting.set");
        assert_eq!(out[0].args["squares"], json!([12]));
        assert_eq!(out[0].args["colour"], json!("ff8c00"));
    }

    /// A quadrant whose squares all go dark needs an explicit clear:
    /// `lighting.set` only addresses the nodes its square list names, so a
    /// stale override would otherwise stay lit for the rest of the game.
    #[test]
    fn a_quadrant_dropping_out_of_the_frame_is_cleared() {
        let mut painter = Painter::new();
        let mut frame = frame_for(Paint::Needed);
        frame.set(12, Paint::Needed);
        painter.diff(&frame, ALL_ONLINE, 1_000, identity);

        let mut next = frame_for(Paint::Needed);
        next.set(40, Paint::Needed);
        let out = painter.diff(&next, ALL_ONLINE, 1_200, identity);
        let names: Vec<&str> = out.iter().map(|c| c.name).collect();
        assert!(names.contains(&"lighting.clear"));
        let clear = out.iter().find(|c| c.name == "lighting.clear").unwrap();
        assert_eq!(clear.args["squares"], json!([12]));
    }

    #[test]
    fn an_unchanged_frame_sends_nothing_until_the_re_assert() {
        let mut painter = Painter::new();
        let mut frame = frame_for(Paint::Needed);
        frame.set(12, Paint::Needed);
        assert!(!painter.diff(&frame, ALL_ONLINE, 1_000, identity).is_empty());
        assert!(painter.diff(&frame, ALL_ONLINE, 1_100, identity).is_empty());
        assert!(
            !painter.diff(&frame, ALL_ONLINE, 2_100, identity).is_empty(),
            "a stray probe from the debug console self-heals within a second"
        );
    }

    #[test]
    fn frames_are_capped_per_second() {
        let mut painter = Painter::new();
        let mut sent = 0;
        for i in 0..20u64 {
            let mut frame = frame_for(Paint::Needed);
            frame.set((i % 40) as u8, Paint::Needed);
            if !painter.diff(&frame, ALL_ONLINE, 1_000 + i * 10, identity).is_empty() {
                sent += 1;
            }
        }
        assert_eq!(sent, MAX_FRAMES_PER_SECOND as usize);
    }

    /// One `node_error code=2` from a bar write retires the bars for the
    /// session and nothing else changes — every value they carry is already on
    /// screen. A quadrant refusing something else is not evidence about bars.
    #[test]
    fn a_node_error_on_a_bar_write_retires_the_bars() {
        let mut painter = Painter::new();
        assert!(!painter.note_node_error(1, 2, false), "not a bar write");
        assert!(!painter.note_node_error(0, 7, true), "a different failure");
        assert!(painter.bars_supported);
        assert!(painter.note_node_error(2, 2, true));
        assert!(!painter.bars_supported);
        assert!(!painter.note_node_error(2, 2, true), "already known");
    }

    #[test]
    fn offline_quadrants_are_never_addressed() {
        let mut painter = Painter::new();
        let mut frame = frame_for(Paint::Needed);
        frame.set(0, Paint::Needed); // node 0
        frame.set(63, Paint::Needed); // node 3
        let out = painter.diff(&frame, [true, true, true, false], 1_000, identity);
        let set = out.iter().find(|c| c.name == "lighting.set").unwrap();
        assert_eq!(set.args["squares"], json!([0]));
    }

    #[test]
    fn the_eval_bar_starts_dead_centre_and_never_bottoms_out() {
        let mut frame = Frame::default();
        frame.eval_bar(0, 0.5, &Palette::default());
        let white = frame.bars[0].iter().filter(|c| c.0 > 0x80).count();
        assert_eq!(white, 8, "the game opens on a 50/50");

        frame.eval_bar(0, 1.0, &Palette::default());
        let white = frame.bars[0].iter().filter(|c| c.0 > 0x80).count();
        assert_eq!(white, 15, "clamped short of mate so it keeps its orientation");

        frame.eval_bar(0, 0.0, &Palette::default());
        let white = frame.bars[0].iter().filter(|c| c.0 > 0x80).count();
        assert_eq!(white, 1);
    }

    #[test]
    fn bar_writes_are_capped_so_the_bus_queue_survives() {
        let mut painter = Painter::new();
        let mut frame = Frame {
            bars_wanted: true,
            ..Frame::default()
        };
        for side in 0..SIDES {
            frame.eval_bar(side, 0.25, &Palette::default());
        }
        let out = painter.diff(&frame, ALL_ONLINE, 1_000, identity);
        let bars = out.iter().filter(|c| c.name == "lighting.bar").count();
        assert_eq!(bars, MAX_BAR_WRITES_PER_FRAME);
    }

    #[test]
    fn a_reversed_half_bar_is_written_backwards() {
        let mut painter = Painter::new();
        painter.bar_map[0][0].reversed = true;
        let mut frame = Frame {
            bars_wanted: true,
            ..Frame::default()
        };
        frame.eval_bar(0, 0.125, &Palette::default()); // one white pixel at the low end
        let out = painter.diff(&frame, ALL_ONLINE, 1_000, identity);
        let first = out.iter().find(|c| c.name == "lighting.bar").unwrap();
        let pixels = first.args["pixels"].as_array().unwrap();
        assert_eq!(pixels[7], json!("e8e8e8"), "reversed puts it at the far end");
    }
}
