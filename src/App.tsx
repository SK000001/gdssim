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
  const [viewportStatus, setViewportStatus] = useState<string>("closed");
  const [loaded, setLoaded] = useState<LoadGdsResult | null>(null);
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("ping")
      .then(setPong)
      .catch((e) => setPong(`error: ${e}`));
  }, []);

  async function openViewport() {
    setError(null);
    try {
      setViewportStatus("opening...");
      const result = await invoke<string>("open_viewport");
      setViewportStatus(result);
    } catch (e) {
      setViewportStatus(`error: ${e}`);
    }
  }

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
      const result = await invoke<LoadGdsResult>("load_gds", { path: picked });
      setLoaded(result);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <main className="app">
      <header className="hdr">
        <h1>GDSSIM</h1>
        <span className="tag">interactive GDS-layout simulator · H2a viewer</span>
      </header>

      <section className="panel">
        <h2>Status</h2>
        <ul>
          <li>IPC ping: <code>{pong}</code></li>
          <li>GPU viewport: <code>{viewportStatus}</code></li>
        </ul>
      </section>

      <section className="panel">
        <h2>GPU Viewport</h2>
        <p>
          Opens a separate Rust-owned native window. Open it first, then
          load a <code>.gds</code> file — the parsed polygons render in
          place of the H1 demo rectangle, fitted to the loaded bbox.
        </p>
        <div className="row">
          <button onClick={openViewport}>Open viewport window</button>
          <button onClick={pickAndLoad}>Open .gds file…</button>
        </div>
      </section>

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

      <footer className="ftr">
        Phase 2a — GDS loading + first viewer. See <code>roadmap.md</code> Track H.
      </footer>
    </main>
  );
}

export default App;
