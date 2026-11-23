//! Color parsing and in-house HEX / RGB / HSL conversions.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub hex: String,
    pub rgb: String,
    pub hsl: String,
}

impl ParsedColor {
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        let hex = if a == 255 {
            format!("#{r:02X}{g:02X}{b:02X}")
        } else {
            format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
        };
        let rgb = if a == 255 {
            format!("rgb({r}, {g}, {b})")
        } else {
            format!("rgba({r}, {g}, {b}, {:.2})", f64::from(a) / 255.0)
        };
        let (h, s, l) = rgb_to_hsl(r, g, b);
        let hsl = if a == 255 {
            format!("hsl({h}, {s}%, {l}%)")
        } else {
            format!("hsla({h}, {s}%, {l}%, {:.2})", f64::from(a) / 255.0)
        };
        Self {
            r,
            g,
            b,
            a,
            hex,
            rgb,
            hsl,
        }
    }
}

/// Convert RGB (0..=255) to HSL `(hue 0..=360, sat 0..=100, light 0..=100)`.
pub fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u16, u8, u8) {
    let rf = f64::from(r) / 255.0;
    let gf = f64::from(g) / 255.0;
    let bf = f64::from(b) / 255.0;

    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let l = (max + min) / 2.0;

    let (h, s) = if delta.abs() < 1e-9 {
        (0.0, 0.0)
    } else {
        let s = delta / (1.0 - (2.0 * l - 1.0).abs());
        let mut h = if (max - rf).abs() < 1e-9 {
            60.0 * (((gf - bf) / delta) % 6.0)
        } else if (max - gf).abs() < 1e-9 {
            60.0 * (((bf - rf) / delta) + 2.0)
        } else {
            60.0 * (((rf - gf) / delta) + 4.0)
        };
        if h < 0.0 {
            h += 360.0;
        }
        (h, s)
    };

    (
        (h.round() as u16) % 360,
        (s * 100.0).round().clamp(0.0, 100.0) as u8,
        (l * 100.0).round().clamp(0.0, 100.0) as u8,
    )
}

/// Convert HSL (`h` degrees 0..=360, `s` 0..=100, `l` 0..=100) to RGB (0..=255).
pub fn hsl_to_rgb(h: f64, s_pct: f64, l_pct: f64) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0);
    let s = (s_pct / 100.0).clamp(0.0, 1.0);
    let l = (l_pct / 100.0).clamp(0.0, 1.0);

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Parse a standalone color string (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb(...)`, `rgba(...)`,
/// `hsl(...)`, `hsla(...)`).
pub fn parse_color(input: &str) -> Option<ParsedColor> {
    let s = input.trim();
    if s.is_empty() || s.len() > 64 || s.contains('\n') {
        return None;
    }

    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }

    static RGB_RE: OnceLock<Regex> = OnceLock::new();
    let rgb_re = RGB_RE.get_or_init(|| {
        Regex::new(r"(?i)^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})(?:\s*,\s*([0-9.]+))?\s*\)$")
            .unwrap()
    });
    if let Some(caps) = rgb_re.captures(s) {
        let r: u16 = caps[1].parse().ok()?;
        let g: u16 = caps[2].parse().ok()?;
        let b: u16 = caps[3].parse().ok()?;
        if r > 255 || g > 255 || b > 255 {
            return None;
        }
        let a = match caps.get(4) {
            Some(m) => {
                let af: f64 = m.as_str().parse().ok()?;
                if !(0.0..=1.0).contains(&af) {
                    return None;
                }
                (af * 255.0).round() as u8
            }
            None => 255,
        };
        return Some(ParsedColor::from_rgba(r as u8, g as u8, b as u8, a));
    }

    static HSL_RE: OnceLock<Regex> = OnceLock::new();
    let hsl_re = HSL_RE.get_or_init(|| {
        Regex::new(r"(?i)^hsla?\(\s*([0-9.]+)(?:deg)?\s*,\s*([0-9.]+)%\s*,\s*([0-9.]+)%(?:\s*,\s*([0-9.]+))?\s*\)$")
            .unwrap()
    });
    if let Some(caps) = hsl_re.captures(s) {
        let h: f64 = caps[1].parse().ok()?;
        let sat: f64 = caps[2].parse().ok()?;
        let light: f64 = caps[3].parse().ok()?;
        if h > 360.0 || sat > 100.0 || light > 100.0 {
            return None;
        }
        let a = match caps.get(4) {
            Some(m) => {
                let af: f64 = m.as_str().parse().ok()?;
                if !(0.0..=1.0).contains(&af) {
                    return None;
                }
                (af * 255.0).round() as u8
            }
            None => 255,
        };
        let (r, g, b) = hsl_to_rgb(h, sat, light);
        return Some(ParsedColor::from_rgba(r, g, b, a));
    }

    None
}

fn parse_hex(hex: &str) -> Option<ParsedColor> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some(ParsedColor::from_rgba(r, g, b, 255))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(ParsedColor::from_rgba(r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(ParsedColor::from_rgba(r, g, b, a))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_rgb_and_hsl() {
        let c1 = parse_color("#FF6B6B").unwrap();
        assert_eq!((c1.r, c1.g, c1.b), (255, 107, 107));
        assert_eq!(c1.hex, "#FF6B6B");
        assert_eq!(c1.rgb, "rgb(255, 107, 107)");
        assert_eq!(c1.hsl, "hsl(0, 100%, 71%)");

        let c2 = parse_color("rgb(255, 107, 107)").unwrap();
        assert_eq!(c2.hex, "#FF6B6B");

        let c3 = parse_color("hsl(0, 100%, 71%)").unwrap();
        assert_eq!(c3.r, 255);
        assert!((i16::from(c3.g) - 107).abs() <= 2);

        let c4 = parse_color("#0f8").unwrap();
        assert_eq!(c4.hex, "#00FF88");

        assert!(parse_color("rgb(300, 0, 0)").is_none());
        assert!(parse_color("#12345").is_none());
        assert!(parse_color("not a color").is_none());
    }
}
