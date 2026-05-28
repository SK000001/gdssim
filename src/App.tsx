import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Viewport, type SceneData, type LayerInfo, type Diag } from "./viewport";
import "./App.css";

type Summary = {
  polygon_count: number;
  bbox_min: [number, number];
  bbox_max: [number, number];
};

function App() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewportRef = useRef<Viewport | null>(null);
  const [gpuReady, setGpuReady] = useState<boolean | null>(null);
  const [loaded, setLoaded] = useState<Summary | null>(null);
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [layers, setLayers] = useState<LayerInfo[]>([]);
  const [hidden, setHidden] = useState<Set<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [diag, setDiag] = useState<Diag | null>(null);

  useEffect(() => {
    if (!canvasRef.current) return;
    const vp = new Viewport(canvasRef.current);
    viewportRef.current = vp;
    vp.onSceneChanged = (newLayers) => {
      setLayers(newLayers);
      setHidden(new Set());
    };
    vp.onDiag = (d) => setDiag(d);
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
    (layer: number) => {
      const next = new Set(hidden);
      if (next.has(layer)) next.delete(layer);
      else next.add(layer);
      setHidden(next);
      viewportRef.current?.setLayerVisible(layer, !next.has(layer));
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
        <span className="tag">H2c · layers + MSAA + edges</span>
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

      {layers.length > 0 && (
        <div className="layerpanel">
          <div className="lp-hd">Layers</div>
          {layers.map((l) => {
            const visible = !hidden.has(l.layer);
            const rgb = `rgb(${Math.round(l.color[0] * 255)}, ${Math.round(l.color[1] * 255)}, ${Math.round(l.color[2] * 255)})`;
            return (
              <label
                key={l.layer}
                className={"lp-row" + (visible ? "" : " off")}
                onClick={(e) => {
                  e.preventDefault();
                  toggleLayer(l.layer);
                }}
              >
                <span className="lp-sw" style={{ background: rgb }} />
                <span className="lp-num">L{l.layer}</span>
                <span className="lp-count">{l.polygon_count}</span>
              </label>
            );
          })}
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
            Mouse wheel = zoom · middle drag = pan · <code>F</code> = fit · <code>+/-</code> = step zoom
          </p>
          {loadedPath && <p className="muted">Last picked: {loadedPath}</p>}
        </div>
      )}
    </div>
  );
}

export default App;
