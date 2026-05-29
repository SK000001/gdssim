//! Switch-level digital simulation (H5) over the extracted transistors.
//!
//! This is the simplest model that gets real gates right: each net is a
//! node, each transistor a gate-controlled switch between its source and
//! drain, and a net's value is decided by which driver (a supply or input
//! net) it can reach through *conducting* switches.
//!
//!   - NMOS conducts when its gate is 1, PMOS when its gate is 0.
//!   - Driver nets (VDD = 1, GND = 0, and the inputs) are held fixed; an
//!     internal net is 1 if it can reach a 1-driver through closed
//!     switches, 0 if it can reach a 0-driver, X if it can reach both
//!     (contention) or neither resolvable driver.
//!   - A gate at X makes its switch *maybe-conducting*: it can spread an
//!     unknown but never a definite 0/1, so a strong driver still wins.
//!
//! Because a switch's state depends on its gate net — which is itself
//! being solved — we iterate to a fixpoint. Combinational logic settles
//! in a handful of passes; an undriven internal net holds its previous
//! value so cross-coupled feedback (latches) can bootstrap, mirroring the
//! tritlogic engine's "seed feedback to 0/last, not null" rule.

use std::collections::HashMap;

use serde::Serialize;

use crate::transistors::{Transistor, TransistorKind};

/// Three-valued logic level on a net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Logic {
    Zero,
    One,
    X,
}

/// Conduction state of a switch given its gate value.
#[derive(Clone, Copy, PartialEq)]
enum Conduction {
    Off,
    On,
    Unknown,
}

fn conduction(kind: TransistorKind, gate: Logic) -> Conduction {
    // NMOS: on at gate=1. PMOS: on at gate=0. Gate X → unknown.
    let on_level = match kind {
        TransistorKind::Nmos => Logic::One,
        TransistorKind::Pmos => Logic::Zero,
    };
    let off_level = match kind {
        TransistorKind::Nmos => Logic::Zero,
        TransistorKind::Pmos => Logic::One,
    };
    if gate == on_level {
        Conduction::On
    } else if gate == off_level {
        Conduction::Off
    } else {
        Conduction::Unknown
    }
}

/// Max fixpoint passes before we give up (oscillation → leave as-is).
const MAX_ITERS: usize = 200;

/// Solve net values given the transistors, the number of (device) nets,
/// and the fixed driver nets (`fixed`: net id → held value — VDD=1,
/// GND=0, and each input's value). Returns one [`Logic`] per net id.
pub fn simulate(
    transistors: &[Transistor],
    net_count: usize,
    fixed: &HashMap<u32, Logic>,
) -> Vec<Logic> {
    let mut val = vec![Logic::X; net_count];
    let mut is_fixed = vec![false; net_count];
    for (&n, &v) in fixed {
        if (n as usize) < net_count {
            val[n as usize] = v;
            is_fixed[n as usize] = true;
        }
    }

    // Switches with both terminals present; gate read fresh each pass.
    let edges: Vec<(usize, usize, usize, TransistorKind)> = transistors
        .iter()
        .filter_map(|t| match (t.source_net, t.drain_net) {
            (Some(s), Some(d)) => Some((s as usize, d as usize, t.gate_net as usize, t.kind)),
            _ => None,
        })
        .collect();

    for _ in 0..MAX_ITERS {
        // Adjacency for this pass: strong (closed switches) and maybe
        // (closed OR unknown). Both undirected.
        let mut strong = vec![Vec::new(); net_count];
        let mut maybe = vec![Vec::new(); net_count];
        for &(s, d, g, kind) in &edges {
            if s >= net_count || d >= net_count {
                continue;
            }
            let gate = val.get(g).copied().unwrap_or(Logic::X);
            match conduction(kind, gate) {
                Conduction::On => {
                    strong[s].push(d);
                    strong[d].push(s);
                    maybe[s].push(d);
                    maybe[d].push(s);
                }
                Conduction::Unknown => {
                    maybe[s].push(d);
                    maybe[d].push(s);
                }
                Conduction::Off => {}
            }
        }

        // Driver sources by value.
        let one_src: Vec<usize> = fixed
            .iter()
            .filter(|(_, &v)| v == Logic::One)
            .map(|(&n, _)| n as usize)
            .collect();
        let zero_src: Vec<usize> = fixed
            .iter()
            .filter(|(_, &v)| v == Logic::Zero)
            .map(|(&n, _)| n as usize)
            .collect();

        // A fixed net is a flood barrier: it can seed a flood (as a
        // source) but a signal can't propagate *through* a clamped node —
        // otherwise a path VDD→…→GND→net would wrongly mark `net`
        // one-reachable. So block expansion at every fixed net except this
        // flood's own sources.
        let block_one = block_mask(&is_fixed, &one_src);
        let block_zero = block_mask(&is_fixed, &zero_src);
        let strong_one = flood(&strong, &one_src, net_count, &block_one);
        let strong_zero = flood(&strong, &zero_src, net_count, &block_zero);
        let maybe_one = flood(&maybe, &one_src, net_count, &block_one);
        let maybe_zero = flood(&maybe, &zero_src, net_count, &block_zero);

        let mut next = val.clone();
        for n in 0..net_count {
            if let Some(&v) = fixed.get(&(n as u32)) {
                next[n] = v;
                continue;
            }
            next[n] = resolve(
                strong_one[n],
                strong_zero[n],
                maybe_one[n],
                maybe_zero[n],
                val[n],
            );
        }

        if next == val {
            return next;
        }
        val = next;
    }
    val
}

/// Decide a net's value from its reachability flags. Strong (closed-path)
/// drivers beat unknown ones; an unknown path can only muddy a result to
/// X, never assert a clean 0/1.
fn resolve(
    strong_one: bool,
    strong_zero: bool,
    maybe_one: bool,
    maybe_zero: bool,
    prev: Logic,
) -> Logic {
    if strong_one && strong_zero {
        Logic::X // hard contention
    } else if strong_one {
        if maybe_zero { Logic::X } else { Logic::One }
    } else if strong_zero {
        if maybe_one { Logic::X } else { Logic::Zero }
    } else if maybe_one || maybe_zero {
        Logic::X // only reachable through unknown switches
    } else {
        prev // undriven → hold (lets feedback latch settle)
    }
}

/// BFS reachability from `sources` over an undirected adjacency list. A
/// node flagged in `block_expand` is recorded as reached but never
/// expanded — used to stop floods from tunnelling through clamped driver
/// nets.
fn flood(
    adj: &[Vec<usize>],
    sources: &[usize],
    net_count: usize,
    block_expand: &[bool],
) -> Vec<bool> {
    let mut seen = vec![false; net_count];
    let mut stack = Vec::new();
    for &s in sources {
        if s < net_count && !seen[s] {
            seen[s] = true;
            stack.push(s);
        }
    }
    while let Some(n) = stack.pop() {
        if block_expand[n] {
            continue;
        }
        for &m in &adj[n] {
            if !seen[m] {
                seen[m] = true;
                stack.push(m);
            }
        }
    }
    seen
}

/// `is_fixed` with each source cleared — the barrier set for a flood from
/// those sources (sources must stay expandable).
fn block_mask(is_fixed: &[bool], sources: &[usize]) -> Vec<bool> {
    let mut b = is_fixed.to_vec();
    for &s in sources {
        if s < b.len() {
            b[s] = false;
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fet(kind: TransistorKind, gate: u32, s: u32, d: u32) -> Transistor {
        Transistor {
            kind,
            poly_index: 0,
            diff_index: 0,
            gate_min: [0.0, 0.0],
            gate_max: [0.0, 0.0],
            gate_net: gate,
            source_net: Some(s),
            drain_net: Some(d),
        }
    }

    // Net ids: 0 GND, 1 VDD, 2 IN, 3 OUT.
    fn inverter_netlist() -> Vec<Transistor> {
        vec![
            fet(TransistorKind::Pmos, 2, 1, 3), // VDD–OUT, gate IN
            fet(TransistorKind::Nmos, 2, 0, 3), // GND–OUT, gate IN
        ]
    }

    fn run(ts: &[Transistor], nets: usize, fixed: &[(u32, Logic)]) -> Vec<Logic> {
        let map: HashMap<u32, Logic> = fixed.iter().copied().collect();
        simulate(ts, nets, &map)
    }

    #[test]
    fn inverter_truth_table() {
        let ts = inverter_netlist();
        // IN = 0 → OUT = 1.
        let v = run(&ts, 4, &[(0, Logic::Zero), (1, Logic::One), (2, Logic::Zero)]);
        assert_eq!(v[3], Logic::One);
        // IN = 1 → OUT = 0.
        let v = run(&ts, 4, &[(0, Logic::Zero), (1, Logic::One), (2, Logic::One)]);
        assert_eq!(v[3], Logic::Zero);
        // IN = X → OUT = X (both switches uncertain).
        let v = run(&ts, 4, &[(0, Logic::Zero), (1, Logic::One), (2, Logic::X)]);
        assert_eq!(v[3], Logic::X);
    }

    #[test]
    fn nand2_truth_table() {
        // Nets: 0 GND, 1 VDD, 2 A, 3 B, 4 OUT, 5 internal (series NMOS mid).
        // Pulldown: GND–[A]–mid–[B]–OUT (series). Pullup: VDD–[A]–OUT and
        // VDD–[B]–OUT (parallel).
        let ts = vec![
            fet(TransistorKind::Nmos, 2, 0, 5), // A: GND–mid
            fet(TransistorKind::Nmos, 3, 5, 4), // B: mid–OUT
            fet(TransistorKind::Pmos, 2, 1, 4), // A: VDD–OUT
            fet(TransistorKind::Pmos, 3, 1, 4), // B: VDD–OUT
        ];
        let base = [(0, Logic::Zero), (1, Logic::One)];
        let cases = [
            (Logic::Zero, Logic::Zero, Logic::One),
            (Logic::Zero, Logic::One, Logic::One),
            (Logic::One, Logic::Zero, Logic::One),
            (Logic::One, Logic::One, Logic::Zero),
        ];
        for (a, b, out) in cases {
            let mut fixed = base.to_vec();
            fixed.push((2, a));
            fixed.push((3, b));
            let v = run(&ts, 6, &fixed);
            assert_eq!(v[4], out, "NAND({a:?},{b:?})");
        }
    }

    #[test]
    fn nor2_truth_table() {
        // Nets: 0 GND, 1 VDD, 2 A, 3 B, 4 OUT, 5 internal (series PMOS mid).
        // Pullup: VDD–[A]–mid–[B]–OUT (series). Pulldown: parallel NMOS.
        let ts = vec![
            fet(TransistorKind::Pmos, 2, 1, 5), // A: VDD–mid
            fet(TransistorKind::Pmos, 3, 5, 4), // B: mid–OUT
            fet(TransistorKind::Nmos, 2, 0, 4), // A: GND–OUT
            fet(TransistorKind::Nmos, 3, 0, 4), // B: GND–OUT
        ];
        let base = [(0, Logic::Zero), (1, Logic::One)];
        let cases = [
            (Logic::Zero, Logic::Zero, Logic::One),
            (Logic::Zero, Logic::One, Logic::Zero),
            (Logic::One, Logic::Zero, Logic::Zero),
            (Logic::One, Logic::One, Logic::Zero),
        ];
        for (a, b, out) in cases {
            let mut fixed = base.to_vec();
            fixed.push((2, a));
            fixed.push((3, b));
            let v = run(&ts, 6, &fixed);
            assert_eq!(v[4], out, "NOR({a:?},{b:?})");
        }
    }

    #[test]
    fn end_to_end_from_extracted_inverter() {
        // Extract the synthetic inverter geometry, then simulate it —
        // ties H4 (extraction) to H5 (sim) on real device nets.
        use crate::tech::Tech;
        use crate::transistors::extract;

        let polys = crate::transistors::tests::inverter();
        let ext = extract(&polys, Tech::default_tech());
        assert_eq!(ext.transistors.len(), 2);

        let pmos = ext.transistors.iter().find(|t| t.kind == TransistorKind::Pmos).unwrap();
        let nmos = ext.transistors.iter().find(|t| t.kind == TransistorKind::Nmos).unwrap();

        // Output = the net shared between a PMOS terminal and an NMOS one.
        let pmos_terms = [pmos.source_net.unwrap(), pmos.drain_net.unwrap()];
        let nmos_terms = [nmos.source_net.unwrap(), nmos.drain_net.unwrap()];
        let out = *pmos_terms.iter().find(|n| nmos_terms.contains(n)).unwrap();
        let vdd = *pmos_terms.iter().find(|n| **n != out).unwrap();
        let gnd = *nmos_terms.iter().find(|n| **n != out).unwrap();
        let input = pmos.gate_net;
        let nets = ext.device_nets.count();

        // IN = 1 → OUT = 0.
        let v = run(&ext.transistors, nets,
            &[(gnd, Logic::Zero), (vdd, Logic::One), (input, Logic::One)]);
        assert_eq!(v[out as usize], Logic::Zero);
        // IN = 0 → OUT = 1.
        let v = run(&ext.transistors, nets,
            &[(gnd, Logic::Zero), (vdd, Logic::One), (input, Logic::Zero)]);
        assert_eq!(v[out as usize], Logic::One);
    }
}
