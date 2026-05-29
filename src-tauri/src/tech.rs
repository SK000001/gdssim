//! Technology file — maps (layer, datatype) → display name + colour.
//!
//! Replaces the old hardcoded 8-slot palette in `viewport.rs`. The
//! default tech is a generic-CMOS guess embedded at build time
//! (`include_str!`); pairs it doesn't list fall back to a deterministic
//! hash colour and a generated name, so any GDS still renders sensibly.
//!
//! This is the forerunner of Track H9 (technology-file abstraction),
//! which will load tech files at runtime (YAML/JSON) so Sky130 / custom
//! processes plug in without code changes.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Electrical role of a layer — drives net connectivity (H3) and
/// transistor extraction (H4). `Via` layers bridge other layers they
/// overlap; `Poly` over `Diffusion` is a transistor (not a connection).
/// `NWell` is a body-tie / classification layer: it carries no signal
/// net but a `Poly`×`Diffusion` gate sitting inside an `NWell` is a
/// PMOS, outside it an NMOS (H4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerRole {
    Conductor,
    Via,
    Poly,
    Diffusion,
    NWell,
    #[default]
    Other,
}

/// One style row: a (layer, datatype) pair with a name, RGB colour, and
/// electrical role.
#[derive(Debug, Clone, Deserialize)]
pub struct LayerStyle {
    pub layer: i16,
    pub datatype: i16,
    pub name: String,
    /// RGB, each component 0..1.
    pub color: [f32; 3],
    #[serde(default)]
    pub role: LayerRole,
}

#[derive(Debug, Deserialize)]
struct TechFile {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    #[serde(default)]
    description: String,
    layers: Vec<LayerStyle>,
}

/// Resolved style for a (layer, datatype): always succeeds via fallback.
pub struct ResolvedStyle {
    pub name: String,
    pub color: [f32; 3],
}

/// A loaded technology: (layer, datatype) → style.
pub struct Tech {
    map: HashMap<(i16, i16), LayerStyle>,
}

static DEFAULT: OnceLock<Tech> = OnceLock::new();

impl Tech {
    /// The built-in tech, parsed once from the embedded JSON.
    pub fn default_tech() -> &'static Tech {
        DEFAULT.get_or_init(|| {
            let src = include_str!("../tech/default.json");
            Tech::from_json(src).unwrap_or_else(|e| {
                log::warn!("default tech parse failed: {e}; using empty tech");
                Tech { map: HashMap::new() }
            })
        })
    }

    pub fn from_json(src: &str) -> Result<Tech, serde_json::Error> {
        let f: TechFile = serde_json::from_str(src)?;
        let map = f
            .layers
            .into_iter()
            .map(|s| ((s.layer, s.datatype), s))
            .collect();
        Ok(Tech { map })
    }

    /// Colour only — cheap, no allocation; used per-vertex while
    /// building the scene.
    pub fn color(&self, layer: i16, datatype: i16) -> [f32; 3] {
        self.map
            .get(&(layer, datatype))
            .map(|s| s.color)
            .unwrap_or_else(|| fallback_color(layer))
    }

    /// Full style (name + colour); used once per layer group + per hit.
    pub fn resolve(&self, layer: i16, datatype: i16) -> ResolvedStyle {
        match self.map.get(&(layer, datatype)) {
            Some(s) => ResolvedStyle { name: s.name.clone(), color: s.color },
            None => ResolvedStyle {
                name: fallback_name(layer, datatype),
                color: fallback_color(layer),
            },
        }
    }

    /// Electrical role of a (layer, datatype); `Other` when unlisted.
    pub fn role(&self, layer: i16, datatype: i16) -> LayerRole {
        self.map
            .get(&(layer, datatype))
            .map(|s| s.role)
            .unwrap_or_default()
    }

    /// Whether this (layer, datatype) bridges layers it overlaps (a
    /// contact / via).
    pub fn is_via(&self, layer: i16, datatype: i16) -> bool {
        self.role(layer, datatype) == LayerRole::Via
    }
}

fn fallback_name(layer: i16, datatype: i16) -> String {
    if datatype == 0 {
        format!("layer {layer}")
    } else {
        format!("layer {layer}/{datatype}")
    }
}

/// Deterministic hash colour for layers the tech doesn't name — same
/// scheme the old `layer_color` used for its out-of-palette fallback.
fn fallback_color(layer: i16) -> [f32; 3] {
    let h = (layer as i32 as u32).wrapping_mul(2654435761);
    let r = ((h >> 16) & 0xff) as f32 / 255.0;
    let g = ((h >> 8) & 0xff) as f32 / 255.0;
    let b = (h & 0xff) as f32 / 255.0;
    [0.4 + 0.5 * r, 0.4 + 0.5 * g, 0.4 + 0.5 * b]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tech_names_known_layers() {
        let t = Tech::default_tech();
        let poly = t.resolve(1, 0);
        assert_eq!(poly.name, "poly");
        assert_eq!(poly.color, [0.45, 0.85, 0.40]);
        // color() agrees with resolve() for a known pair.
        assert_eq!(t.color(1, 0), poly.color);
        // roles parsed from default.json.
        assert_eq!(t.role(1, 0), LayerRole::Poly);
        assert_eq!(t.role(2, 0), LayerRole::Diffusion);
        assert!(t.is_via(3, 0)); // contact
        assert!(t.is_via(7, 0)); // via
        assert!(!t.is_via(4, 0)); // metal1 is a conductor
    }

    #[test]
    fn unknown_pairs_fall_back() {
        let t = Tech::default_tech();
        // Unknown datatype on a known layer → fallback (NOT the poly style).
        let s = t.resolve(1, 5);
        assert_eq!(s.name, "layer 1/5");
        assert_eq!(s.color, fallback_color(1));
        // Unknown layer, datatype 0 → "layer N".
        assert_eq!(t.resolve(42, 0).name, "layer 42");
        // Fallback is deterministic.
        assert_eq!(t.color(42, 0), fallback_color(42));
        // Unknown pairs are role Other (inert cross-layer).
        assert_eq!(t.role(42, 0), LayerRole::Other);
        assert!(!t.is_via(42, 0));
    }
}
