import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

function App() {
  const [pong, setPong] = useState<string>("(not pinged)");
  const [viewportStatus, setViewportStatus] = useState<string>("closed");

  useEffect(() => {
    invoke<string>("ping")
      .then(setPong)
      .catch((e) => setPong(`error: ${e}`));
  }, []);

  async function openViewport() {
    try {
      setViewportStatus("opening...");
      const result = await invoke<string>("open_viewport");
      setViewportStatus(result);
    } catch (e) {
      setViewportStatus(`error: ${e}`);
    }
  }

  return (
    <main className="app">
      <header className="hdr">
        <h1>GDSSIM</h1>
        <span className="tag">interactive GDS-layout simulator · H1 scaffold</span>
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
          Opens a separate Rust-owned native window that renders a clear color
          + one rectangle in world coords via <code>wgpu</code>. This proves
          the GPU pipeline is wired end-to-end. Embedding into this webview is
          a later milestone.
        </p>
        <button onClick={openViewport}>Open viewport window</button>
      </section>

      <footer className="ftr">
        Phase 1 — foundation. See <code>roadmap.md</code> Track H.
      </footer>
    </main>
  );
}

export default App;
