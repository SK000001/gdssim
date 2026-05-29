import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Viewport,
  type SceneData,
  type LayerInfo,
  type Diag,
  type PolygonHit,
  type Transistor,
  type Logic,
} from "./viewport";
import "./App.css";

type Summary = {
  polygon_count: number;
  bbox_min: [number, number];
  bbox_max: [number, number];
  top_cell: string;
  cell_names: string[];
};

/** Rings per device-net id, from the `device_nets_geometry` IPC. */
type DeviceGeom = [number, number][][][];

/** Stable key for a (layer, datatype) technology style. */
const styleKey = (layer: number, datatype: number) => `${layer}/${datatype}`;

/** Even-odd point-in-ring test (database units). */
function pointInRing(ring: [number, number][], px: number, py: number): boolean {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const xi = ring[i][0], yi = ring[i][1];
    const xj = ring[j][0], yj = ring[j][1];
    if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }
  return inside;
}

function ringArea(ring: [number, number][]): number {
  let a = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    a += ring[j][0] * ring[i][1] - ring[i][0] * ring[j][1];
  }
  return Math.abs(a) * 0.5;
}

/** Device net under a world point: the one with the smallest containing
 *  ring (mirrors the Rust hit-test's "most specific feature wins"). */
function deviceNetAt(geom: DeviceGeom, x: number, y: number): number | null {
  let best: number | null = null;
  let bestArea = Infinity;
  geom.forEach((rings, net) => {
    for (const r of rings) {
      if (pointInRing(r, x, y)) {
        const a = ringArea(r);
        if (a < bestArea) {
          bestArea = a;
          best = net;
        }
      }
    }
  });
  return best;
}

const SIM_RED: [number, number, number, number] = [0.95, 0.25, 0.25, 0.55];
const SIM_BLUE: [number, number, number, number] = [0.3, 0.55, 1.0, 0.5];
const SIM_GREY: [number, number, number, number] = [0.6, 0.6, 0.6, 0.4];
const SIM_GREEN: [number, number, number, number] = [0.35, 1.0, 0.45, 0.55];

function App() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewportRef = useRef<Viewport | null>(null);
  const [gpuReady, setGpuReady] = useState<boolean | null>(null);
  const [loaded, setLoaded] = useState<Summary | null>(null);
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [layers, setLayers] = useState<LayerInfo[]>([]);
  // Visibility is keyed per (layer, datatype) style — see styleKey().
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [diag, setDiag] = useState<Diag | null>(null);
  const [selected, setSelected] = useState<PolygonHit | null>(null);
  const [transistors, setTransistors] = useState<Transistor[]>([]);
  const [activeFet, setActiveFet] = useState<number | null>(null);
  // --- H5b digital sim ---
  const [simMode, setSimMode] = useState(false);
  const [deviceGeom, setDeviceGeom] = useState<DeviceGeom>([]);
  const [vddNet, setVddNet] = useState<number | null>(null);
  const [gndNet, setGndNet] = useState<number | null>(null);
  const [inputs, setInputs] = useState<Record<number, 0 | 1>>({});
  const [simValues, setSimValues] = useState<Logic[]>([]);
  const [selectedNet, setSelectedNet] = useState<number | null>(null);
  // Refs so the (once-bound) onPick handler reads live sim state.
  const simModeRef = useRef(false);
  const deviceGeomRef = useRef<DeviceGeom>([]);
  simModeRef.current = simMode;
  deviceGeomRef.current = deviceGeom;

  useEffect(() => {
    if (!canvasRef.current) return;
    const vp = new Viewport(canvasRef.current);
    viewportRef.current = vp;
    vp.onSceneChanged = (newLayers) => {
      setLayers(newLayers);
      setHidden(new Set());
      setSelected(null);
      setActiveFet(null);
    };
    vp.onDiag = (d) => setDiag(d);
    vp.onPick = async (world) => {
      // Sim mode: clicks select a device net to assign a role to.
      if (simModeRef.current) {
        const net = deviceNetAt(deviceGeomRef.current, world[0], world[1]);
        setSelectedNet(net);
        vp.setHighlight(net != null ? deviceGeomRef.current[net] : null);
        return;
      }
      try {
        const hit = await invoke<PolygonHit | null>("hit_test", {
          x: world[0],
          y: world[1],
        });
        setSelected(hit);
        setActiveFet(null);
        if (!hit) {
          vp.setHighlight(null);
          return;
        }
        // Highlight the whole electrically-connected net, not just the
        // clicked polygon (H3).
        const rings = await invoke<[number, number][][]>("net_rings", {
          netId: hit.net_id,
        });
        vp.setHighlight(rings.length > 0 ? rings : [hit.points]);
      } catch (e) {
        setError(String(e));
      }
    };
    vp.init()
      .then((ok) => {
        setGpuReady(ok);
        if (!ok) {
          setError(
            "WebGPU is not available in this WebView2 build. Update Edge/WebView2 runtime."
          );
        }
      })
      .catch((e) => {
        setGpuReady(false);
        setError(`WebGPU init failed: ${e}`);
      });
    return () => {
      vp.destroy();
      viewportRef.current = null;
    };
  }, []);

  const toggleLayer = useCallback(
    (layer: number, datatype: number) => {
      const key = styleKey(layer, datatype);
      const next = new Set(hidden);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      setHidden(next);
      viewportRef.current?.setLayerVisible(layer, datatype, !next.has(key));
    },
    [hidden]
  );

  // Click a device row → highlight its gate channel rectangle plus its
  // gate / source / drain device-nets (H4 refined connectivity).
  const selectFet = useCallback(
    async (i: number) => {
      const t = transistors[i];
      const vp = viewportRef.current;
      if (!t || !vp) return;
      setActiveFet(i);
      setSelected(null);
      const rings: [number, number][][] = [
        [
          [t.gate_min[0], t.gate_min[1]],
          [t.gate_max[0], t.gate_min[1]],
          [t.gate_max[0], t.gate_max[1]],
          [t.gate_min[0], t.gate_max[1]],
        ],
      ];
      const nets = [t.gate_net, t.source_net, t.drain_net].filter(
        (n): n is number => n != null
      );
      for (const n of nets) {
        const r = await invoke<[number, number][][]>("device_net_rings", { netId: n });
        rings.push(...r);
      }
      vp.setHighlight(rings);
    },
    [transistors]
  );

  // Assign a role to a device net, keeping VDD/GND unique and a net in at
  // most one role.
  const assignRole = useCallback((net: number, role: "vdd" | "gnd" | "in" | "clear") => {
    setVddNet((v) => (role === "vdd" ? net : v === net ? null : v));
    setGndNet((g) => (role === "gnd" ? net : g === net ? null : g));
    setInputs((ins) => {
      const next = { ...ins };
      delete next[net];
      if (role === "in") next[net] = 0;
      return next;
    });
  }, []);

  const runSim = useCallback(async () => {
    const vp = viewportRef.current;
    if (!vp || deviceGeom.length === 0) return;
    const fixed: [number, Logic][] = [];
    if (vddNet != null) fixed.push([vddNet, "one"]);
    if (gndNet != null) fixed.push([gndNet, "zero"]);
    for (const [k, v] of Object.entries(inputs)) fixed.push([Number(k), v ? "one" : "zero"]);
    if (fixed.length === 0) {
      setSimValues([]);
      vp.setSimOverlay(null);
      return;
    }
    const values = await invoke<Logic[]>("simulate_nets", { fixed });
    setSimValues(values);
    const ones: [number, number][][] = [];
    const zeros: [number, number][][] = [];
    const xs: [number, number][][] = [];
    values.forEach((val, net) => {
      const rings = deviceGeom[net];
      if (!rings) return;
      if (val === "one") ones.push(...rings);
      else if (val === "zero") zeros.push(...rings);
      else xs.push(...rings);
    });
    // Conducting transistors → a green glow on their gate channel.
    const onFets: [number, number][][] = [];
    for (const t of transistors) {
      const g = values[t.gate_net];
      const on = (t.kind === "nmos" && g === "one") || (t.kind === "pmos" && g === "zero");
      if (!on) continue;
      const [x0, y0] = t.gate_min;
      const [x1, y1] = t.gate_max;
      onFets.push([
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
      ]);
    }
    vp.setSimOverlay([
      { color: SIM_GREY, rings: xs },
      { color: SIM_BLUE, rings: zeros },
      { color: SIM_RED, rings: ones, flow: true },
      { color: SIM_GREEN, rings: onFets },
    ]);
  }, [vddNet, gndNet, inputs, deviceGeom, transistors]);

  // Re-solve whenever assignments change; clear the overlay when sim mode
  // is off.
  useEffect(() => {
    if (simMode) runSim();
    else viewportRef.current?.setSimOverlay(null);
  }, [simMode, runSim]);

  async function pickAndLoad() {
    setError(null);
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "GDS-II", extensions: ["gds", "gds2", "gdsii"] }],
      });
      if (!picked || typeof picked !== "string") return;
      setLoadedPath(picked);
      setLoading(true);
      const scene = await invoke<SceneData>("load_gds", { path: picked });
      viewportRef.current?.loadScene(scene);
      setLoaded({
        polygon_count: scene.polygon_count,
        bbox_min: scene.bbox_min,
        bbox_max: scene.bbox_max,
        top_cell: scene.top_cell,
        cell_names: scene.cell_names,
      });
      setTransistors(await invoke<Transistor[]>("transistors"));
      setDeviceGeom(await invoke<DeviceGeom>("device_nets_geometry"));
      // Reset sim assignments for the new layout.
      setVddNet(null);
      setGndNet(null);
      setInputs({});
      setSimValues([]);
      setSelectedNet(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="root">
      <canvas ref={canvasRef} className="canvas" />

      <div className="topbar">
        <button className="primary" onClick={pickAndLoad} disabled={loading || gpuReady === false}>
          {loading ? "Loading…" : "Open .gds file…"}
        </button>
        <button onClick={() => viewportRef.current?.fitView()} disabled={!loaded}>
          Fit (F)
        </button>
        <button
          onClick={() => {
            setSimMode((m) => !m);
            setSelected(null);
            setActiveFet(null);
            viewportRef.current?.setHighlight(null);
          }}
          disabled={!loaded}
          className={simMode ? "primary" : ""}
        >
          {simMode ? "Exit sim" : "Simulate"}
        </button>
        <span className="title">GDSSIM</span>
        <span className="tag">
          {simMode ? "H5 · click a net → assign VDD/GND/input" : "H4 · transistors · click a device"}
        </span>
        <span className="spacer" />
        {loaded && (
          <span className="summary">
            {loaded.polygon_count} polys
          </span>
        )}
        {diag && (
          <span className="summary" title="canvas / msaa / layers / frames / err">
            {diag.canvasW}×{diag.canvasH} · msaa {diag.msaaW}×{diag.msaaH} · L{diag.layers} · f{diag.frames}
            {diag.err ? ` · ERR: ${diag.err}` : ""}
          </span>
        )}
      </div>

      {loaded && (
        <div className="cellpanel">
          <div className="lp-hd">Cells</div>
          <div className="muted" style={{ marginBottom: "0.4rem" }}>
            top: <code>{loaded.top_cell}</code> · {loaded.cell_names.length} cells in lib
          </div>
          <details>
            <summary className="muted" style={{ cursor: "pointer", fontSize: "0.8rem" }}>
              all cells
            </summary>
            <ul style={{ margin: "0.3rem 0 0", padding: "0 0 0 1rem", fontSize: "0.8rem", color: "#bbb" }}>
              {loaded.cell_names.map((n) => (
                <li key={n} style={{ fontFamily: "ui-monospace, monospace" }}>
                  {n === loaded.top_cell ? <strong>{n}</strong> : n}
                </li>
              ))}
            </ul>
          </details>
        </div>
      )}

      {layers.length > 0 && (
        <div className="layerpanel">
          <div className="lp-hd">Layers</div>
          {layers.map((l) => {
            const key = styleKey(l.layer, l.datatype);
            const visible = !hidden.has(key);
            const rgb = `rgb(${Math.round(l.color[0] * 255)}, ${Math.round(l.color[1] * 255)}, ${Math.round(l.color[2] * 255)})`;
            return (
              <label
                key={key}
                className={"lp-row" + (visible ? "" : " off")}
                title={`layer ${l.layer} · datatype ${l.datatype}`}
                onClick={(e) => {
                  e.preventDefault();
                  toggleLayer(l.layer, l.datatype);
                }}
              >
                <span className="lp-sw" style={{ background: rgb }} />
                <span className="lp-name">{l.name}</span>
                <span className="lp-num">
                  {l.layer}/{l.datatype}
                </span>
                <span className="lp-count">{l.polygon_count}</span>
              </label>
            );
          })}
        </div>
      )}

      {transistors.length > 0 && (
        <div className="devpanel">
          <div className="lp-hd">Devices · {transistors.length}</div>
          {transistors.map((t, i) => (
            <div
              key={i}
              className={"fet-row" + (activeFet === i ? " active" : "")}
              title="Highlight gate channel + gate/source/drain nets"
              onClick={() => selectFet(i)}
            >
              <span className={"fet-kind " + t.kind}>{t.kind.toUpperCase()}</span>
              <span className="fet-nets">
                G{t.gate_net} · S{t.source_net ?? "?"} · D{t.drain_net ?? "?"}
              </span>
            </div>
          ))}
        </div>
      )}

      {simMode && (
        <div className="simpanel">
          <div className="lp-hd">
            Simulation
            <span className="sim-legend">
              <span className="sim-dot one" /> 1
              <span className="sim-dot zero" /> 0
              <span className="sim-dot x" /> X
              <span className="sim-dot on" /> on
            </span>
          </div>

          <div className="sim-roles">
            <div className="sim-role">
              <span className="sim-tag vdd">VDD</span>
              <span>{vddNet != null ? `net #${vddNet}` : <em className="muted">unset</em>}</span>
            </div>
            <div className="sim-role">
              <span className="sim-tag gnd">GND</span>
              <span>{gndNet != null ? `net #${gndNet}` : <em className="muted">unset</em>}</span>
            </div>
            <div className="sim-role">
              <span className="sim-tag in">IN</span>
              {Object.keys(inputs).length === 0 ? (
                <em className="muted">none</em>
              ) : (
                <span className="sim-inputs">
                  {Object.entries(inputs).map(([k, v]) => (
                    <button
                      key={k}
                      className={"sim-input " + (v ? "hi" : "lo")}
                      title={`net #${k} — click to toggle`}
                      onClick={() => setInputs((s) => ({ ...s, [Number(k)]: s[Number(k)] ? 0 : 1 }))}
                    >
                      #{k}={v}
                    </button>
                  ))}
                </span>
              )}
            </div>
          </div>

          {selectedNet != null ? (
            <div className="sim-sel">
              <div>
                net <strong>#{selectedNet}</strong>
                {simValues[selectedNet] && (
                  <span className={"sim-val " + simValues[selectedNet]}>
                    {" "}= {simValues[selectedNet].toUpperCase()}
                  </span>
                )}
              </div>
              <div className="sim-btns">
                <button onClick={() => assignRole(selectedNet, "vdd")}>VDD</button>
                <button onClick={() => assignRole(selectedNet, "gnd")}>GND</button>
                <button onClick={() => assignRole(selectedNet, "in")}>Input</button>
                <button onClick={() => assignRole(selectedNet, "clear")}>Clear</button>
              </div>
            </div>
          ) : (
            <div className="muted sim-hint">Click a net in the layout to assign a role.</div>
          )}
        </div>
      )}

      {selected && (
        <div className="inspector">
          <div className="lp-hd">
            Selection
            <button
              className="insp-x"
              title="Clear selection"
              onClick={() => {
                setSelected(null);
                viewportRef.current?.setHighlight(null);
              }}
            >
              ×
            </button>
          </div>
          <dl className="insp-grid">
            <dt>Style</dt>
            <dd>{selected.name}</dd>
            <dt>Layer</dt>
            <dd>{selected.layer}</dd>
            <dt>Datatype</dt>
            <dd>{selected.datatype}</dd>
            <dt>Net</dt>
            <dd>
              #{selected.net_id}
              {selected.net_size > 1 ? ` (${selected.net_size} polys)` : ""}
            </dd>
            <dt>Vertices</dt>
            <dd>{selected.point_count}</dd>
            <dt>Area</dt>
            <dd>{selected.area.toLocaleString()} du²</dd>
            <dt>BBox</dt>
            <dd>
              {Math.round(selected.bbox_max[0] - selected.bbox_min[0])} ×{" "}
              {Math.round(selected.bbox_max[1] - selected.bbox_min[1])} du
            </dd>
            <dt>Origin</dt>
            <dd>
              ({Math.round(selected.bbox_min[0])}, {Math.round(selected.bbox_min[1])})
            </dd>
          </dl>
        </div>
      )}

      {error && (
        <div className="overlay overlay-err">
          <strong>Error</strong>
          <code>{error}</code>
        </div>
      )}

      {!loaded && !error && gpuReady && (
        <div className="overlay">
          <p>Open a <code>.gds</code> file to start.</p>
          <p className="muted">
            Left click = inspect + highlight net · wheel = zoom · middle drag = pan · <code>F</code> = fit · <code>+/-</code> = step zoom
          </p>
          {loadedPath && <p className="muted">Last picked: {loadedPath}</p>}
        </div>
      )}
    </div>
  );
}

export default App;
