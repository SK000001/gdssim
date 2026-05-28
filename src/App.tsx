import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

type LoadGdsResult = {
  polygon_count: number;
  layers: number[];
  bbox_min: [number, number];
  bbox_max: [number, number];
};

function App() {
  const [pong, setPong] = useState<string>("(not pinged)");
  const [loaded, setLoaded] = useState<LoadGdsResult | null>(null);
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    invoke<string>("ping")
      .then(setPong)
      .catch((e) => setPong(`error: ${e}`));
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
      const result = await invoke<LoadGdsResult>("load_gds", { path: picked });
      setLoaded(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <main className="app">
      <header className="topbar">
        <button className="primary" onClick={pickAndLoad} disabled={loading}>
          {loading ? "Loading…" : "Open .gds file…"}
        </button>
        <span className="title">GDSSIM</span>
        <span className="tag">H2b · viewer + camera</span>
      </header>

      {loaded && (
        <section className="panel">
          <h2>Loaded</h2>
          <ul>
            <li>File: <code>{loadedPath}</code></li>
            <li>Polygons: <code>{loaded.polygon_count}</code></li>
            <li>Layers: <code>{loaded.layers.join(", ")}</code></li>
            <li>
              Bbox: <code>
                ({loaded.bbox_min[0].toFixed(0)}, {loaded.bbox_min[1].toFixed(0)})
                – ({loaded.bbox_max[0].toFixed(0)}, {loaded.bbox_max[1].toFixed(0)})
              </code>
            </li>
          </ul>
        </section>
      )}

      {error && (
        <section className="panel panel-err">
          <h2>Error</h2>
          <code>{error}</code>
        </section>
      )}

      {!loaded && !error && (
        <section className="panel">
          <h2>Viewport controls</h2>
          <ul>
            <li>Mouse wheel — zoom (cursor-anchored)</li>
            <li>Middle drag — pan</li>
            <li><code>F</code> — fit to scene</li>
            <li><code>+ / -</code> — step zoom</li>
          </ul>
          <p className="muted">
            The viewport opens automatically with the app. Pick a <code>.gds</code>
            file above to load polygons. IPC: <code>{pong}</code>.
          </p>
        </section>
      )}
    </main>
  );
}

export default App;
