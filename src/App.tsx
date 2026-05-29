import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Viewport, type SceneData, type LayerInfo, type Diag, type PolygonHit } from "./viewport";
import "./App.css";

type Summary = {
  polygon_count: number;
  bbox_min: [number, number];
  bbox_max: [number, number];
  top_cell: string;
  cell_names: string[];
};

/** Stable key for a (layer, datatype) technology style. */
const styleKey = (layer: number, datatype: number) => `${layer}/${datatype}`;

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

  useEffect(() => {
    if (!canvasRef.current) return;
    const vp = new Viewport(canvasRef.current);
    viewportRef.current = vp;
    vp.onSceneChanged = (newLayers) => {
      setLayers(newLayers);
      setHidden(new Set());
      setSelected(null);
    };
    vp.onDiag = (d) => setDiag(d);
    vp.onPick = async (world) => {
      try {
        const hit = await invoke<PolygonHit | null>("hit_test", {
          x: world[0],
          y: world[1],
        });
        setSelected(hit);
        vp.setHighlight(hit ? hit.points : null);
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
        <span className="title">GDSSIM</span>
        <span className="tag">H2d · tech-file colours</span>
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
            Left click = inspect · wheel = zoom · middle drag = pan · <code>F</code> = fit · <code>+/-</code> = step zoom
          </p>
          {loadedPath && <p className="muted">Last picked: {loadedPath}</p>}
        </div>
      )}
    </div>
  );
}

export default App;
