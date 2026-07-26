//! Everything worth changing without a redeploy, and the machinery that lets a
//! phone at the venue change it.
//!
//! # Why this is its own module
//!
//! The deploy is a CapRover build from git, so an environment variable is not an
//! operator control — reaching one means a dashboard edit and a container
//! restart at best, a push and a rebuild at worst. Either way it is minutes, and
//! either way the process dies. So the rule here is:
//!
//! > If it might plausibly need to change once the board is on the table, it has
//! > to be reachable from the admin rail **and** it has to survive a restart.
//!
//! # The three lifetimes
//!
//! Configuration in this system is not one thing, and treating it as one is the
//! trap — a projector colour and a sensor threshold have nothing in common
//! except being literals.
//!
//! - [`Tunables`] — detection calibration. Revised together during rehearsal,
//!   only meaningful relative to each other, thrown away between venues.
//! - [`Rules`] — how the game is played and scored. Presentation-shaping;
//!   changing one never invalidates a calibration.
//! - [`VenueProfile`] — physical facts about this board in this room: which way
//!   it is mounted, which sensors lie, which half-bar is which. This is the
//!   expensive artifact. It is *derived* from live state rather than duplicating
//!   it, so there is exactly one owner of every value.
//!
//! # What is deliberately not here
//!
//! The bus budget (`paint::MAX_FRAMES_PER_SECOND` and friends) is physics, not
//! preference — one 38400-baud line and an eight-deep queue. Making it adjustable
//! invites someone to "speed the board up" into queue overflow at 11 pm. The
//! win-probability curve and the piece values are semantics, not settings. And
//! credentials stay in the environment, never in a file the app rewrites.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::observe::SQUARES;

/// Where the venue profile is written. Deliberately **not** `/tmp`: on a
/// container platform `/tmp` does not survive the container being replaced,
/// which is precisely the event the profile exists to survive.
pub fn config_path() -> String {
    std::env::var("CONFIG_PATH").unwrap_or_else(|_| "/srv/venue.json".to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// Group A — detection calibration. Every field is live-editable and every field
/// is clamped to [`SETTINGS`] on the way in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tunables {
    pub settle_ms: u64,
    pub autostart_stable_ms: u64,
    pub unknown_tolerance: usize,
    pub tier3_max_distance: f64,
    pub tier3_margin: f64,
    pub tier3_neighbour_credit: f64,
    pub tier3_unreadable_penalty: f64,
    pub tier3_empty_penalty: f64,
    pub tier3_polarity_credit: f64,
    /// Consecutive settles a square must disagree with the game before it is
    /// retired as a liar.
    pub auto_mask_streak: u8,
}

impl Default for Tunables {
    fn default() -> Self {
        let d = infer_defaults();
        Tunables {
            settle_ms: env_u64("SETTLE_MS", 700),
            autostart_stable_ms: env_u64("AUTOSTART_STABLE_MS", 1500),
            unknown_tolerance: env_u64("UNKNOWN_TOLERANCE", 0) as usize,
            tier3_max_distance: env_f64("TIER3_MAX_DISTANCE", d.max_distance),
            tier3_margin: env_f64("TIER3_MARGIN", d.margin),
            tier3_neighbour_credit: env_f64("TIER3_NEIGHBOUR_CREDIT", d.neighbour_credit),
            tier3_unreadable_penalty: env_f64("TIER3_UNREADABLE_PENALTY", d.unreadable_penalty),
            tier3_empty_penalty: env_f64("TIER3_EMPTY_PENALTY", d.empty_penalty),
            tier3_polarity_credit: env_f64("TIER3_POLARITY_CREDIT", d.polarity_credit),
            auto_mask_streak: env_u64("AUTO_MASK_STREAK", 2) as u8,
        }
    }
}

fn infer_defaults() -> super::infer::Params {
    super::infer::Params::default()
}

impl Tunables {
    /// The matcher's view of this calibration.
    pub fn params(&self) -> super::infer::Params {
        super::infer::Params {
            max_distance: self.tier3_max_distance,
            margin: self.tier3_margin,
            neighbour_credit: self.tier3_neighbour_credit,
            unreadable_penalty: self.tier3_unreadable_penalty,
            empty_penalty: self.tier3_empty_penalty,
            polarity_credit: self.tier3_polarity_credit,
        }
    }
}

/// Group C — how the game is played and scored.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub max_ply: usize,
    pub draw_band_cp: i32,
    pub countdown_ms: u64,
    pub autopilot_interval_ms: u64,
    /// Relaxes the admin gate for player-facing actions, for a second unauthed
    /// tablet at the board. A decision you only make once you see where the
    /// players are standing, so it is live rather than an environment variable.
    pub open_controls: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            max_ply: env_u64("MAX_PLY", 10) as usize,
            draw_band_cp: env_u64("DRAW_BAND_CP", 40) as i32,
            countdown_ms: env_u64("COUNTDOWN_MS", 3000),
            autopilot_interval_ms: env_u64("AUTOPILOT_INTERVAL_MS", 4000),
            open_controls: env_bool("GAME_OPEN_CONTROLS", false),
        }
    }
}

/// Group D — the lighting palette, as `0xRRGGBB`.
///
/// "Amber = still needed, white = placed" *is* the setup experience, and on
/// Basic-tier firmware it is the only physical output the demo has. Whether
/// `ff8c00` actually reads as amber through a wooden board at LED brightness 48
/// under venue lighting is unknowable until the board is on the table.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub alert: u32,
    pub needed: u32,
    pub focus: u32,
    pub sweep: u32,
    pub bar_white: u32,
    pub bar_black: u32,
    /// RGB565 written to AVR config keys 7 and 8 so both polarities render as
    /// one "a piece is here" colour instead of random per-piece green/blue.
    pub neutralised_rgb565: u16,
    /// RGB565 restored to keys 7 and 8 on abort, putting the board back to its
    /// bring-up behaviour.
    pub restore_pos_rgb565: u16,
    pub restore_neg_rgb565: u16,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            alert: 0xd0_20_20,
            needed: 0xff_8c_00,
            focus: 0x2a_6a_d0,
            sweep: 0xf0_f0_f0,
            bar_white: 0xe8_e8_e8,
            bar_black: 0x30_34_52,
            neutralised_rgb565: 0xffff,
            restore_pos_rgb565: 0x07e0,
            restore_neg_rgb565: 0x001f,
        }
    }
}

/// One half of one edge bar, as recorded by the venue mapping tool.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BarSlot {
    pub node: u8,
    pub strip: char,
    pub reversed: bool,
}

/// Group B — the expensive artifact: ten minutes of venue calibration.
///
/// This is a *serialisation view*, assembled from live state on the way out and
/// applied back into it on the way in. It deliberately does not own anything —
/// the observer owns rotation and masks, the painter owns the bar map — so there
/// is no second copy to drift.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VenueProfile {
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub rotation: u8,
    /// Manually masked squares only. Auto-masks are per-game evidence, not
    /// calibration, and are deliberately not carried across a restart.
    #[serde(default)]
    pub masked: Vec<u8>,
    #[serde(default)]
    pub bar_map: Option<[[BarSlot; 2]; 4]>,
    #[serde(default)]
    pub detect_mode: Option<String>,
    /// Which board edges carry the eval bar and which carry the turn indicator.
    /// If the audience happens to face a turn edge they watch a solid colour
    /// block all night and never see the eval bar, which is the narrative device
    /// the whole demo hangs on — and which edge that is cannot be known until
    /// the room is set up.
    #[serde(default = "default_eval_sides")]
    pub eval_sides: Vec<u8>,
    #[serde(default = "default_turn_sides")]
    pub turn_sides: Vec<u8>,
}

fn default_eval_sides() -> Vec<u8> {
    vec![0, 2]
}

fn default_turn_sides() -> Vec<u8> {
    vec![1, 3]
}

impl VenueProfile {
    pub fn masked_array(&self) -> [bool; SQUARES] {
        let mut out = [false; SQUARES];
        for &sq in &self.masked {
            if (sq as usize) < SQUARES {
                out[sq as usize] = true;
            }
        }
        out
    }
}

/// What gets written to disk. Versioned so that a schema change across a restart
/// is a clean discard with a log line, not a panic or a half-applied profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    pub version: u32,
    #[serde(default)]
    pub tunables: Option<Tunables>,
    #[serde(default)]
    pub rules: Option<Rules>,
    #[serde(default)]
    pub palette: Option<Palette>,
    #[serde(default)]
    pub profile: Option<VenueProfile>,
}

pub const CONFIG_VERSION: u32 = 1;

/// Which kind of control the admin rail should render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
    Bool,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Bool => "bool",
        }
    }
}

/// One self-describing setting.
///
/// This table is the single source of truth for what is adjustable and what
/// range is sane. It is serialised into every `game.state`, so the admin rail
/// builds itself from it — which means adding a setting is one line here instead
/// of a change in the Rust default, the setter, the broadcast, the TypeScript
/// interface and the Svelte markup. It also makes it structurally impossible for
/// the UI to offer a value the server will refuse, because both read this table.
pub struct Setting {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub unit: &'static str,
    /// False for settings that only take effect on the next game.
    pub live: bool,
    pub help: &'static str,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "settle_ms",
        group: "detection",
        label: "settle window",
        kind: Kind::Int,
        min: 100.0,
        max: 3000.0,
        step: 50.0,
        unit: "ms",
        live: true,
        help: "How long the board must hold still before a move is read. Raise it if players hover.",
    },
    Setting {
        key: "autostart_stable_ms",
        group: "detection",
        label: "auto-start hold",
        kind: Kind::Int,
        min: 300.0,
        max: 8000.0,
        step: 100.0,
        unit: "ms",
        live: true,
        help: "How long a correct setup must hold before the countdown begins.",
    },
    Setting {
        key: "unknown_tolerance",
        group: "detection",
        label: "unknown squares allowed",
        kind: Kind::Int,
        min: 0.0,
        max: 8.0,
        step: 1.0,
        unit: "",
        live: true,
        help: "How many unreadable target squares auto-start will forgive.",
    },
    Setting {
        key: "tier3_max_distance",
        group: "detection",
        label: "fuzzy match ceiling",
        kind: Kind::Float,
        min: 0.0,
        // Deliberately capped below the unreadable-destination penalty: past
        // that point a masked or offline destination stops being a veto and one
        // dead sensor silently swallows every move played onto it.
        max: 1.75,
        step: 0.25,
        unit: "",
        live: true,
        help: "How wrong the board may look and still commit a move. Above 1.75 a dead sensor starts eating moves.",
    },
    Setting {
        key: "tier3_margin",
        group: "detection",
        label: "fuzzy match margin",
        kind: Kind::Float,
        min: 0.0,
        max: 4.0,
        step: 0.25,
        unit: "",
        live: true,
        help: "How far clear the best guess must be from the runner-up before it commits.",
    },
    Setting {
        key: "tier3_neighbour_credit",
        group: "detection",
        label: "off-centre credit",
        kind: Kind::Float,
        min: 0.0,
        max: 2.0,
        step: 0.25,
        unit: "",
        live: true,
        help: "Cost of a piece landing half a square over and reading next door.",
    },
    Setting {
        key: "tier3_unreadable_penalty",
        group: "detection",
        label: "unreadable destination",
        kind: Kind::Float,
        min: 0.0,
        max: 4.0,
        step: 0.25,
        unit: "",
        live: true,
        help: "Cost of committing onto a square nothing can confirm. Keep above the fuzzy match ceiling.",
    },
    Setting {
        key: "tier3_empty_penalty",
        group: "detection",
        label: "empty destination",
        kind: Kind::Float,
        min: 0.0,
        max: 4.0,
        step: 0.25,
        unit: "",
        live: true,
        help: "Cost of committing onto a square that reads empty.",
    },
    Setting {
        key: "tier3_polarity_credit",
        group: "detection",
        label: "polarity confirmation",
        kind: Kind::Float,
        min: 0.0,
        max: 2.0,
        step: 0.25,
        unit: "",
        live: true,
        help: "Credit when the magnet fingerprint confirms the same piece moved.",
    },
    Setting {
        key: "auto_mask_streak",
        group: "detection",
        label: "auto-mask after",
        kind: Kind::Int,
        min: 1.0,
        max: 10.0,
        step: 1.0,
        unit: " settles",
        live: true,
        help: "Consecutive disagreements before a square is retired as broken.",
    },
    Setting {
        key: "max_ply",
        group: "rules",
        label: "plies per game",
        kind: Kind::Int,
        min: 2.0,
        max: 40.0,
        step: 2.0,
        unit: "",
        live: false,
        help: "Total half-moves. Takes effect on the next new game.",
    },
    Setting {
        key: "draw_band_cp",
        group: "rules",
        label: "draw band",
        kind: Kind::Int,
        min: 0.0,
        max: 300.0,
        step: 10.0,
        unit: "cp",
        live: true,
        help: "How much the evaluation must swing before someone is declared the winner.",
    },
    Setting {
        key: "countdown_ms",
        group: "rules",
        label: "countdown",
        kind: Kind::Int,
        min: 0.0,
        max: 10000.0,
        step: 500.0,
        unit: "ms",
        live: true,
        help: "The 3-2-1 before play begins once the board is built.",
    },
    Setting {
        key: "autopilot_interval_ms",
        group: "rules",
        label: "autopilot pace",
        kind: Kind::Int,
        min: 500.0,
        max: 60000.0,
        step: 500.0,
        unit: "ms",
        live: true,
        help: "Seconds per move when the board plays itself as an attract loop.",
    },
    Setting {
        key: "open_controls",
        group: "rules",
        label: "unauthed player controls",
        kind: Kind::Bool,
        min: 0.0,
        max: 1.0,
        step: 1.0,
        unit: "",
        live: true,
        help: "Let an unauthenticated tablet at the board start, move and answer prompts.",
    },
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Clamps a raw value to the advertised range. Every write goes through here, so
/// the server can never hold a value the rail could not have produced — and a
/// fat-fingered number under stage lights is recoverable instead of fatal.
pub fn clamp(key: &str, value: f64) -> Option<f64> {
    let s = setting(key)?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(s.min, s.max))
}

/// The schema the admin rail renders itself from.
pub fn schema() -> Value {
    Value::Array(
        SETTINGS
            .iter()
            .map(|s| {
                json!({
                    "key": s.key,
                    "group": s.group,
                    "label": s.label,
                    "kind": s.kind.as_str(),
                    "min": s.min,
                    "max": s.max,
                    "step": s.step,
                    "unit": s.unit,
                    "live": s.live,
                    "help": s.help,
                })
            })
            .collect(),
    )
}

/// Current values, keyed the same way as [`SETTINGS`], so the rail can pair one
/// with the other without knowing any field names.
pub fn values(tun: &Tunables, rules: &Rules) -> BTreeMap<&'static str, Value> {
    let mut out = BTreeMap::new();
    out.insert("settle_ms", json!(tun.settle_ms));
    out.insert("autostart_stable_ms", json!(tun.autostart_stable_ms));
    out.insert("unknown_tolerance", json!(tun.unknown_tolerance));
    out.insert("tier3_max_distance", json!(tun.tier3_max_distance));
    out.insert("tier3_margin", json!(tun.tier3_margin));
    out.insert("tier3_neighbour_credit", json!(tun.tier3_neighbour_credit));
    out.insert(
        "tier3_unreadable_penalty",
        json!(tun.tier3_unreadable_penalty),
    );
    out.insert("tier3_empty_penalty", json!(tun.tier3_empty_penalty));
    out.insert("tier3_polarity_credit", json!(tun.tier3_polarity_credit));
    out.insert("auto_mask_streak", json!(tun.auto_mask_streak));
    out.insert("max_ply", json!(rules.max_ply));
    out.insert("draw_band_cp", json!(rules.draw_band_cp));
    out.insert("countdown_ms", json!(rules.countdown_ms));
    out.insert("autopilot_interval_ms", json!(rules.autopilot_interval_ms));
    out.insert("open_controls", json!(rules.open_controls));
    out
}

/// Applies one clamped setting. Returns false for an unknown key or an
/// unusable value, which the caller turns into `invalid_args` rather than a
/// silent no-op — a control that accepts a value and ignores it is worse than
/// one that refuses it.
pub fn apply(tun: &mut Tunables, rules: &mut Rules, key: &str, raw: &Value) -> bool {
    let Some(spec) = setting(key) else {
        return false;
    };
    if spec.kind == Kind::Bool {
        let Some(b) = raw.as_bool() else { return false };
        match key {
            "open_controls" => rules.open_controls = b,
            _ => return false,
        }
        return true;
    }
    let Some(v) = raw.as_f64() else { return false };
    let Some(v) = clamp(key, v) else { return false };
    match key {
        "settle_ms" => tun.settle_ms = v as u64,
        "autostart_stable_ms" => tun.autostart_stable_ms = v as u64,
        "unknown_tolerance" => tun.unknown_tolerance = v as usize,
        "tier3_max_distance" => tun.tier3_max_distance = v,
        "tier3_margin" => tun.tier3_margin = v,
        "tier3_neighbour_credit" => tun.tier3_neighbour_credit = v,
        "tier3_unreadable_penalty" => tun.tier3_unreadable_penalty = v,
        "tier3_empty_penalty" => tun.tier3_empty_penalty = v,
        "tier3_polarity_credit" => tun.tier3_polarity_credit = v,
        "auto_mask_streak" => tun.auto_mask_streak = v as u8,
        "max_ply" => rules.max_ply = v as usize,
        "draw_band_cp" => rules.draw_band_cp = v as i32,
        "countdown_ms" => rules.countdown_ms = v as u64,
        "autopilot_interval_ms" => rules.autopilot_interval_ms = v as u64,
        _ => return false,
    }
    true
}

/// Clamps a whole struct, for values arriving from a config file written by an
/// older build or edited by hand.
pub fn clamp_all(tun: &mut Tunables, rules: &mut Rules) {
    let snapshot = values(tun, rules);
    for (key, value) in snapshot {
        if setting(key).map(|s| s.kind) == Some(Kind::Bool) {
            continue;
        }
        if let Some(v) = value.as_f64().and_then(|v| clamp(key, v)) {
            apply(tun, rules, key, &json!(v));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_a_value_and_every_value_a_setting() {
        let (tun, rules) = (Tunables::default(), Rules::default());
        let values = values(&tun, &rules);
        for s in SETTINGS {
            assert!(values.contains_key(s.key), "{} has no value", s.key);
        }
        for key in values.keys() {
            assert!(setting(key).is_some(), "{key} has no setting descriptor");
        }
    }

    #[test]
    fn defaults_sit_inside_their_own_advertised_ranges() {
        let (tun, rules) = (Tunables::default(), Rules::default());
        for (key, value) in values(&tun, &rules) {
            let s = setting(key).unwrap();
            if s.kind == Kind::Bool {
                continue;
            }
            let v = value.as_f64().unwrap();
            assert!(
                v >= s.min && v <= s.max,
                "{key} default {v} is outside {}..{}",
                s.min,
                s.max
            );
        }
    }

    /// The whole point of the range table: a value the rail could not produce
    /// must not be reachable over the wire either.
    #[test]
    fn nonsense_is_clamped_rather_than_stored() {
        let (mut tun, mut rules) = (Tunables::default(), Rules::default());

        assert!(apply(&mut tun, &mut rules, "settle_ms", &json!(0)));
        assert_eq!(tun.settle_ms, 100, "clamped up to the floor");

        assert!(apply(&mut tun, &mut rules, "settle_ms", &json!(4_000_000_000u64)));
        assert_eq!(tun.settle_ms, 3000, "clamped down to the ceiling");

        assert!(apply(&mut tun, &mut rules, "tier3_max_distance", &json!(1e9)));
        assert_eq!(tun.tier3_max_distance, 1.75);

        assert!(!apply(&mut tun, &mut rules, "settle_ms", &json!(f64::NAN)));
        assert!(!apply(&mut tun, &mut rules, "no_such_key", &json!(1)));
        assert!(!apply(&mut tun, &mut rules, "settle_ms", &json!("nope")));
    }

    /// The fuzzy-match ceiling must stay below the unreadable-destination
    /// penalty, or a masked square silently stops vetoing commits onto it.
    #[test]
    fn the_fuzzy_ceiling_cannot_be_raised_past_the_unreadable_veto() {
        let tun = Tunables::default();
        let ceiling = setting("tier3_max_distance").unwrap().max;
        assert!(
            ceiling < tun.tier3_unreadable_penalty,
            "ceiling {ceiling} must stay under the {} veto",
            tun.tier3_unreadable_penalty
        );
    }

    #[test]
    fn bools_only_accept_bools() {
        let (mut tun, mut rules) = (Tunables::default(), Rules::default());
        assert!(!apply(&mut tun, &mut rules, "open_controls", &json!(1)));
        assert!(apply(&mut tun, &mut rules, "open_controls", &json!(true)));
        assert!(rules.open_controls);
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let profile = VenueProfile {
            device_id: Some("arcade-chess-001".to_string()),
            rotation: 2,
            masked: vec![12, 27],
            bar_map: None,
            detect_mode: Some("suggest".to_string()),
            eval_sides: vec![1, 3],
            turn_sides: vec![0, 2],
        };
        let text = serde_json::to_string(&ConfigFile {
            version: CONFIG_VERSION,
            tunables: Some(Tunables::default()),
            rules: Some(Rules::default()),
            palette: Some(Palette::default()),
            profile: Some(profile),
        })
        .unwrap();
        let back: ConfigFile = serde_json::from_str(&text).unwrap();
        let profile = back.profile.unwrap();
        assert_eq!(profile.rotation, 2);
        assert_eq!(profile.masked, vec![12, 27]);
        assert_eq!(profile.eval_sides, vec![1, 3]);
        assert!(profile.masked_array()[27]);
        assert!(!profile.masked_array()[28]);
    }

    /// A file from an older build must degrade to defaults, not blow up.
    #[test]
    fn a_partial_file_fills_in_from_defaults() {
        let back: ConfigFile = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert!(back.tunables.is_none());
        assert!(back.profile.is_none());

        let back: ConfigFile =
            serde_json::from_str(r#"{"version":1,"profile":{"rotation":3}}"#).unwrap();
        let profile = back.profile.unwrap();
        assert_eq!(profile.rotation, 3);
        assert_eq!(profile.eval_sides, vec![0, 2], "missing fields default");
        assert!(profile.masked.is_empty());
    }
}
