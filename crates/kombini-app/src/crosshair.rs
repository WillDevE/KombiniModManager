//! Crosshair settings, their console commands and on-screen geometry, and
//! the managed blocks Kombini keeps inside a profile's autoexec.
//!
//! The convar set, the game's defaults and the layout math (1080p layout
//! units, pip centres `4 + gap` from the middle, outlines growing by
//! `gap + 2 * border`) follow Grimoire's crosshair editor (MIT, esoc).

/// Every crosshair convar the game reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crosshair {
    pub pip_gap: f32,
    pub pip_gap_static: bool,
    pub pip_height: f32,
    pub pip_width: f32,
    pub pip_opacity: f32,
    pub pip_outline_border: f32,
    pub pip_outline_gap: f32,
    pub pip_outline_opacity: f32,
    pub dot_size: f32,
    pub dot_opacity: f32,
    pub dot_outline_border: f32,
    pub dot_outline_gap: f32,
    pub dot_outline_opacity: f32,
    pub color: [u8; 3],
    pub outline_color: [u8; 3],
    pub disable_hero_specific: bool,
}

impl Crosshair {
    /// A fresh install's crosshair.
    pub const GAME: Crosshair = Crosshair {
        pip_gap: 4.,
        pip_gap_static: false,
        pip_height: 16.,
        pip_width: 2.,
        pip_opacity: 0.5,
        pip_outline_border: 1.,
        pip_outline_gap: 1.,
        pip_outline_opacity: 0.7,
        dot_size: 4.,
        dot_opacity: 0.7,
        dot_outline_border: 2.,
        dot_outline_gap: 2.,
        dot_outline_opacity: 0.7,
        color: [255, 255, 255],
        outline_color: [0, 0, 0],
        disable_hero_specific: false,
    };

    pub const PRESETS: [(&'static str, Crosshair); 4] = [
        ("Game default", Self::GAME),
        (
            "Green cross",
            Crosshair {
                pip_gap: 5.,
                pip_gap_static: true,
                pip_height: 10.,
                pip_opacity: 1.,
                pip_outline_gap: 0.,
                pip_outline_opacity: 1.,
                dot_opacity: 0.,
                dot_outline_opacity: 0.,
                color: [0, 255, 0],
                ..Self::GAME
            },
        ),
        (
            "Dot only",
            Crosshair {
                pip_opacity: 0.,
                pip_outline_opacity: 0.,
                dot_size: 4.,
                dot_opacity: 1.,
                dot_outline_border: 1.,
                dot_outline_gap: 0.,
                dot_outline_opacity: 1.,
                color: [0, 255, 255],
                ..Self::GAME
            },
        ),
        (
            "Small static",
            Crosshair {
                pip_gap: 2.,
                pip_gap_static: true,
                pip_height: 6.,
                pip_opacity: 1.,
                pip_outline_gap: 0.,
                pip_outline_opacity: 1.,
                dot_size: 2.,
                dot_opacity: 1.,
                dot_outline_border: 1.,
                dot_outline_gap: 0.,
                dot_outline_opacity: 1.,
                color: [255, 255, 0],
                ..Self::GAME
            },
        ),
    ];

    /// The full convar set, so applying overrides the in-game editor.
    pub fn commands(&self) -> String {
        let b = |v: bool| if v { "true" } else { "false" };
        [
            format!("citadel_crosshair_pip_gap {}", self.pip_gap.trunc()),
            format!(
                "citadel_crosshair_pip_gap_static {}",
                b(self.pip_gap_static)
            ),
            format!("citadel_crosshair_pip_height {}", self.pip_height.trunc()),
            format!("citadel_crosshair_pip_width {}", self.pip_width.trunc()),
            format!("citadel_crosshair_pip_opacity {:.2}", self.pip_opacity),
            format!(
                "citadel_crosshair_pip_outline_border {}",
                self.pip_outline_border.trunc()
            ),
            format!(
                "citadel_crosshair_pip_outline_gap {}",
                self.pip_outline_gap.trunc()
            ),
            format!(
                "citadel_crosshair_pip_outline_opacity {:.2}",
                self.pip_outline_opacity
            ),
            format!("citadel_crosshair_dot_size {}", self.dot_size.trunc()),
            format!("citadel_crosshair_dot_opacity {:.2}", self.dot_opacity),
            format!(
                "citadel_crosshair_dot_outline_border {}",
                self.dot_outline_border.trunc()
            ),
            format!(
                "citadel_crosshair_dot_outline_gap {}",
                self.dot_outline_gap.trunc()
            ),
            format!(
                "citadel_crosshair_dot_outline_opacity {:.2}",
                self.dot_outline_opacity
            ),
            format!("citadel_crosshair_color_r {}", self.color[0]),
            format!("citadel_crosshair_color_g {}", self.color[1]),
            format!("citadel_crosshair_color_b {}", self.color[2]),
            format!(
                "citadel_crosshair_outline_color_r {}",
                self.outline_color[0]
            ),
            format!(
                "citadel_crosshair_outline_color_g {}",
                self.outline_color[1]
            ),
            format!(
                "citadel_crosshair_outline_color_b {}",
                self.outline_color[2]
            ),
            format!(
                "citadel_crosshair_disable_hero_specific_crosshairs {}",
                b(self.disable_hero_specific)
            ),
        ]
        .join("\n")
    }

    /// Applies `name value` pairs (autoexec lines or machine_convars.vcfg
    /// entries) on top of `self`. Unknown names are ignored.
    pub fn apply_convars<'a>(
        mut self,
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        for (name, value) in pairs {
            let value = value.trim().trim_matches('"');
            let num = value.parse::<f32>().ok();
            let flag = match value {
                "1" | "true" => Some(true),
                "0" | "false" => Some(false),
                _ => None,
            };
            let byte = num.map(|n| n.clamp(0., 255.) as u8);
            match name.trim().to_ascii_lowercase().as_str() {
                "citadel_crosshair_pip_gap" => self.pip_gap = num.unwrap_or(self.pip_gap),
                "citadel_crosshair_pip_gap_static" => {
                    self.pip_gap_static = flag.unwrap_or(self.pip_gap_static)
                }
                "citadel_crosshair_pip_height" => self.pip_height = num.unwrap_or(self.pip_height),
                "citadel_crosshair_pip_width" => self.pip_width = num.unwrap_or(self.pip_width),
                "citadel_crosshair_pip_opacity" => {
                    self.pip_opacity = num.unwrap_or(self.pip_opacity)
                }
                "citadel_crosshair_pip_outline_border" => {
                    self.pip_outline_border = num.unwrap_or(self.pip_outline_border)
                }
                "citadel_crosshair_pip_outline_gap" => {
                    self.pip_outline_gap = num.unwrap_or(self.pip_outline_gap)
                }
                "citadel_crosshair_pip_outline_opacity" => {
                    self.pip_outline_opacity = num.unwrap_or(self.pip_outline_opacity)
                }
                "citadel_crosshair_dot_size" => self.dot_size = num.unwrap_or(self.dot_size),
                "citadel_crosshair_dot_opacity" => {
                    self.dot_opacity = num.unwrap_or(self.dot_opacity)
                }
                "citadel_crosshair_dot_outline_border" => {
                    self.dot_outline_border = num.unwrap_or(self.dot_outline_border)
                }
                "citadel_crosshair_dot_outline_gap" => {
                    self.dot_outline_gap = num.unwrap_or(self.dot_outline_gap)
                }
                "citadel_crosshair_dot_outline_opacity" => {
                    self.dot_outline_opacity = num.unwrap_or(self.dot_outline_opacity)
                }
                "citadel_crosshair_color_r" => self.color[0] = byte.unwrap_or(self.color[0]),
                "citadel_crosshair_color_g" => self.color[1] = byte.unwrap_or(self.color[1]),
                "citadel_crosshair_color_b" => self.color[2] = byte.unwrap_or(self.color[2]),
                "citadel_crosshair_outline_color_r" => {
                    self.outline_color[0] = byte.unwrap_or(self.outline_color[0])
                }
                "citadel_crosshair_outline_color_g" => {
                    self.outline_color[1] = byte.unwrap_or(self.outline_color[1])
                }
                "citadel_crosshair_outline_color_b" => {
                    self.outline_color[2] = byte.unwrap_or(self.outline_color[2])
                }
                "citadel_crosshair_disable_hero_specific_crosshairs" => {
                    self.disable_hero_specific = flag.unwrap_or(self.disable_hero_specific)
                }
                _ => {}
            }
        }
        self
    }

    /// Reads autoexec-style lines (`name value`).
    pub fn from_commands(base: Crosshair, text: &str) -> Crosshair {
        base.apply_convars(text.lines().filter_map(|l| {
            let l = l.split("//").next()?.trim();
            l.split_once(char::is_whitespace)
        }))
    }

    /// Reads the game's saved settings (`"name" "value"` in machine_convars.vcfg).
    pub fn from_machine_convars(base: Crosshair, text: &str) -> Crosshair {
        base.apply_convars(text.lines().filter_map(|l| {
            let mut parts = l.split('"').filter(|p| !p.trim().is_empty());
            let name = parts.next()?.trim();
            let value = parts.next()?;
            name.starts_with("citadel_crosshair_")
                .then_some((name, value))
        }))
    }

    /// On-screen geometry at `scale` (display height / 1080), in device px
    /// relative to the screen centre.
    pub fn layout(&self, scale: f32) -> Layout {
        let t = |v: f32| v.trunc();
        let dev = |v: f32| (v * scale).round();
        let edge = |centre: f32, size: f32| (centre * scale - size / 2.).round();
        let d = 4. + t(self.pip_gap);
        let (w, h) = (t(self.pip_width), t(self.pip_height));
        let specs: Vec<(f32, f32, f32, f32)> = if w > 0. && h > 0. {
            vec![(0., -d, w, h), (0., d, w, h), (-d, 0., h, w), (d, 0., h, w)]
        } else {
            Vec::new()
        };
        let boxed = |(cx, cy, w, h): (f32, f32, f32, f32), grow: f32| {
            let (dw, dh) = (dev(w + grow), dev(h + grow));
            Rect {
                x: edge(cx, dw),
                y: edge(cy, dh),
                w: dw,
                h: dh,
            }
        };
        let pip_grow = t(self.pip_outline_gap) + 2. * t(self.pip_outline_border);
        Layout {
            pips: specs.iter().map(|s| boxed(*s, 0.)).collect(),
            pip_outlines: specs.iter().map(|s| boxed(*s, pip_grow)).collect(),
            pip_outline_border: dev(t(self.pip_outline_border)),
            dot_size: dev(t(self.dot_size)),
            dot_outline_size: dev(t(self.dot_size)
                + t(self.dot_outline_gap)
                + 2. * t(self.dot_outline_border)),
            dot_outline_border: dev(t(self.dot_outline_border)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub pips: Vec<Rect>,
    pub pip_outlines: Vec<Rect>,
    pub pip_outline_border: f32,
    pub dot_size: f32,
    pub dot_outline_size: f32,
    pub dot_outline_border: f32,
}

/// Replaces (or appends) a block between `// >>> Kombini {name}` and
/// `// <<< Kombini {name}` in `text`. An empty `body` removes the block.
pub fn set_block(text: &str, name: &str, body: &str) -> String {
    let start = format!("// >>> Kombini {name} (managed by Kombini Mod Manager)");
    let end = format!("// <<< Kombini {name}");
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    let mut replaced = false;
    let block = format!("{start}\n{}\n{end}", body.trim_end());
    for line in text.lines() {
        if line
            .trim_start()
            .starts_with(&format!("// >>> Kombini {name}"))
        {
            inside = true;
            continue;
        }
        if inside {
            if line.trim_start().starts_with(&end) {
                inside = false;
                if !body.trim().is_empty() && !replaced {
                    out.push(&block);
                    replaced = true;
                }
            }
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if !replaced && !body.trim().is_empty() {
        if !s.trim().is_empty() {
            s = s.trim_end().to_owned();
            s.push_str("\n\n");
        }
        s.push_str(&block);
    }
    s = s.trim_end().to_owned();
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// The body of block `name` in `text`, if present.
pub fn get_block(text: &str, name: &str) -> Option<String> {
    let mut inside = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line
            .trim_start()
            .starts_with(&format!("// >>> Kombini {name}"))
        {
            inside = true;
            continue;
        }
        if inside
            && line
                .trim_start()
                .starts_with(&format!("// <<< Kombini {name}"))
        {
            return Some(lines.join("\n"));
        }
        if inside {
            lines.push(line);
        }
    }
    None
}

/// Whether `command` (e.g. `fps_max 0`) is an active line of `text`.
pub fn has_command(text: &str, command: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let want = norm(command);
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .any(|l| norm(l) == want)
}

/// One library command.
pub struct LibCommand {
    pub name: &'static str,
    pub command: &'static str,
    pub description: &'static str,
}

/// Curated Deadlock autoexec commands, from Grimoire's command presets (MIT).
pub const COMMAND_LIBRARY: &[(&str, &[LibCommand])] = &[
    (
        "Performance",
        &[
            LibCommand {
                name: "Uncap FPS",
                command: "fps_max 0",
                description: "Remove the frame rate limit",
            },
            LibCommand {
                name: "Cap FPS 144",
                command: "fps_max 144",
                description: "Cap to 144 FPS",
            },
            LibCommand {
                name: "Cap FPS 240",
                command: "fps_max 240",
                description: "Cap to 240 FPS",
            },
            LibCommand {
                name: "Low latency (NVIDIA)",
                command: "r_low_latency 2",
                description: "NVIDIA Reflex low latency",
            },
            LibCommand {
                name: "Engine low latency",
                command: "engine_low_latency_sleep_after_client_tick true",
                description: "Reduce input lag",
            },
        ],
    ),
    (
        "Network",
        &[LibCommand {
            name: "Max network rate",
            command: "rate 1000000",
            description: "Maximum network update rate",
        }],
    ),
    (
        "HUD and UI",
        &[
            LibCommand {
                name: "New health bars",
                command: "citadel_unit_status_use_new true",
                description: "New-style health bars",
            },
            LibCommand {
                name: "Skip post-match survey",
                command: "deadlock_post_match_survey_disabled true",
                description: "No survey after matches",
            },
            LibCommand {
                name: "Offscreen damage indicators",
                command: "citadel_damage_offscreen_indicator_disabled false",
                description: "Show offscreen damage indicators",
            },
        ],
    ),
    (
        "Minimap",
        &[
            LibCommand {
                name: "Faster minimap",
                command: "minimap_update_rate_hz 60",
                description: "Update the minimap at 60 Hz",
            },
            LibCommand {
                name: "Larger click radius",
                command: "citadel_minimap_unit_click_radius 200",
                description: "Easier to click units on the minimap",
            },
            LibCommand {
                name: "Larger player icons",
                command: "citadel_minimap_player_width 6.5",
                description: "Bigger player icons",
            },
            LibCommand {
                name: "Larger own icon",
                command: "citadel_minimap_local_player_width 10.0",
                description: "Make your own icon larger",
            },
            LibCommand {
                name: "Thicker ziplines",
                command: "citadel_minimap_zip_line_thickness 2",
                description: "More visible ziplines",
            },
        ],
    ),
    (
        "Matchmaking",
        &[
            LibCommand {
                name: "Prefer solo queue",
                command: "mm_prefer_solo_only 1",
                description: "Prefer matches with solo players",
            },
            LibCommand {
                name: "Region: North America",
                command: "citadel_region_override 0",
                description: "Force NA servers",
            },
            LibCommand {
                name: "Region: Europe",
                command: "citadel_region_override 1",
                description: "Force EU servers",
            },
            LibCommand {
                name: "Region: Asia",
                command: "citadel_region_override 2",
                description: "Force Asia servers",
            },
            LibCommand {
                name: "Region: automatic",
                command: "citadel_region_override -1",
                description: "Automatic region",
            },
        ],
    ),
    (
        "Mouse",
        &[LibCommand {
            name: "1:1 ADS sensitivity",
            command: "zoom_sensitivity_ratio 0.818933027098955175",
            description: "Match aim-down-sights to hip-fire sensitivity",
        }],
    ),
];

#[cfg(test)]
mod tests {
    use super::{Crosshair, get_block, has_command, set_block};

    #[test]
    fn commands_round_trip() {
        let x = Crosshair::PRESETS[1].1;
        let back = Crosshair::from_commands(Crosshair::GAME, &x.commands());
        assert_eq!(back, x);
    }

    #[test]
    fn reads_machine_convars() {
        let vcfg = "\"config\"\n{\n\t\"citadel_crosshair_color_r\"\t\t\"12\"\n\t\"citadel_crosshair_pip_gap_static\"\t\t\"true\"\n\t\"volume\"\t\t\"0.5\"\n}\n";
        let x = Crosshair::from_machine_convars(Crosshair::GAME, vcfg);
        assert_eq!(x.color[0], 12);
        assert!(x.pip_gap_static);
    }

    #[test]
    fn layout_matches_the_game_math() {
        let l = Crosshair::GAME.layout(1.0);
        // Top pip: centre 4 + 4 = 8 above, 2 x 16.
        assert_eq!(l.pips[0].w, 2.);
        assert_eq!(l.pips[0].h, 16.);
        assert_eq!(l.pips[0].y, -16.);
        assert_eq!(l.pip_outlines[0].w, 2. + 1. + 2.);
        assert_eq!(l.dot_outline_size, 4. + 2. + 4.);
        assert!(
            Crosshair {
                pip_width: 0.,
                ..Crosshair::GAME
            }
            .layout(1.)
            .pips
            .is_empty()
        );
    }

    #[test]
    fn managed_blocks() {
        let base = "fps_max 0\n";
        let a = set_block(base, "crosshair", "x 1");
        assert!(a.starts_with("fps_max 0\n\n// >>> Kombini crosshair"));
        assert_eq!(get_block(&a, "crosshair").as_deref(), Some("x 1"));
        let b = set_block(&a, "crosshair", "x 2\ny 3");
        assert_eq!(get_block(&b, "crosshair").as_deref(), Some("x 2\ny 3"));
        assert_eq!(b.matches(">>> Kombini").count(), 1);
        let c = set_block(&b, "crosshair", "");
        assert_eq!(c, "fps_max 0\n");
        assert!(has_command(&b, "fps_max  0"));
        assert!(!has_command("// fps_max 0", "fps_max 0"));
    }
}
