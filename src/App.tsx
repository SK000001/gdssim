import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Viewport, type SceneData } from "./viewport";
import "./App.css";

type Summary = {
  polygon_count: number;
  layers: number[];
  bbox_min: [number, number];
  bbox_max: [number, number];
};

function App() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewportRef = useRef<Viewport | null>(null);
  const [gpuReady, setGpuReady] = useState<boolean | null>(null);
  const [loaded, setLoaded] = useState<Summary | null>(null);
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!canvasRef.current) return;
    const vp = new Viewport(canvasRef.current);
    viewportRef.current = vp;
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
        layers: scene.layers,
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
        <span className="tag">H1.5 · single-window WebGPU</span>
        <span className="spacer" />
        {loaded && (
          <span className="summary">
            {loaded.polygon_count} polys · layers {loaded.layers.join(", ")}
          </span>
        )}
      </div>

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
