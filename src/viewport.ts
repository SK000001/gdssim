// WebGPU viewport: renders polygons returned by `load_gds` into a
// <canvas>. Rust does parsing/flatten/triangulation; this file owns
// the GPU pipeline + camera + input.
//
// H2c: scene split per layer so visibility toggles map to draw-call
// skips. Two pipelines per layer: a fill (triangle-list) and an edge
// (line-list, brighter colour) so thin features stay visible even
// when they drop sub-pixel at low zoom. 4× MSAA on the colour target
// kills the jaggies on diagonal waveguide S-bends.

import shaderSrc from "./viewport.wgsl?raw";

export type LayerData = {
  layer: number;
  color: [number, number, number];
  polygon_count: number;
  vertices: number[];
  triangle_indices: number[];
  edge_indices: number[];
};

export type SceneData = {
  polygon_count: number;
  bbox_min: [number, number];
  bbox_max: [number, number];
  layers: LayerData[];
};

export type LayerInfo = {
  layer: number;
  color: [number, number, number];
  polygon_count: number;
};

export type Diag = {
  canvasW: number;
  canvasH: number;
  msaaW: number;
  msaaH: number;
  layers: number;
  frames: number;
  err: string | null;
};

type GpuLayer = {
  layer: number;
  visible: boolean;
  vbuf: GPUBuffer;
  tris: GPUBuffer;
  triCount: number;
  edges: GPUBuffer;
  edgeCount: number;
};

const VERTEX_FLOATS = 5;
const MSAA_SAMPLES = 4;

export class Viewport {
  private canvas: HTMLCanvasElement;
  private device!: GPUDevice;
  private ctx!: GPUCanvasContext;
  private format!: GPUTextureFormat;
  private fillPipeline!: GPURenderPipeline;
  private edgePipeline!: GPURenderPipeline;
  private uniformBuf!: GPUBuffer;
  private uniformBg!: GPUBindGroup;
  private msaaTex: GPUTexture | null = null;
  private msaaView: GPUTextureView | null = null;
  private gpuLayers: GpuLayer[] = [];
  private cam = {
    center: [0, 0] as [number, number],
    halfH: 150,
    bbox: { min: [0, 0] as [number, number], max: [0, 0] as [number, number] },
  };
  private cursor: [number, number] = [0, 0];
  private panning = false;
  private panLast: [number, number] = [0, 0];
  private destroyed = false;
  /** Called whenever scene metadata changes (after load). */
  public onSceneChanged: ((layers: LayerInfo[]) => void) | null = null;
  public onDiag: ((d: Diag) => void) | null = null;
  private diag: Diag = { canvasW: 0, canvasH: 0, msaaW: 0, msaaH: 0, layers: 0, frames: 0, err: null };

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
  }

  async init(): Promise<boolean> {
    if (!("gpu" in navigator)) return false;
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return false;
    this.device = await adapter.requestDevice();

    const ctx = this.canvas.getContext("webgpu");
    if (!ctx) return false;
    this.ctx = ctx;
    this.format = navigator.gpu.getPreferredCanvasFormat();
    this.ctx.configure({
      device: this.device,
      format: this.format,
      alphaMode: "opaque",
    });

    const shader = this.device.createShaderModule({ code: shaderSrc });

    const bgLayout = this.device.createBindGroupLayout({
      entries: [
        {
          binding: 0,
          visibility: GPUShaderStage.VERTEX,
          buffer: { type: "uniform" },
        },
      ],
    });

    this.uniformBuf = this.device.createBuffer({
      size: 64, // mat4x4<f32>
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
    this.uniformBg = this.device.createBindGroup({
      layout: bgLayout,
      entries: [{ binding: 0, resource: { buffer: this.uniformBuf } }],
    });

    const pipelineLayout = this.device.createPipelineLayout({
      bindGroupLayouts: [bgLayout],
    });
    const vertexState: GPUVertexState = {
      module: shader,
      entryPoint: "vs_main",
      buffers: [
        {
          arrayStride: VERTEX_FLOATS * 4,
          attributes: [
            { shaderLocation: 0, offset: 0, format: "float32x2" },
            { shaderLocation: 1, offset: 8, format: "float32x3" },
          ],
        },
      ],
    };

    this.fillPipeline = this.device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: vertexState,
      fragment: {
        module: shader,
        entryPoint: "fs_fill",
        targets: [{ format: this.format }],
      },
      primitive: { topology: "triangle-list", frontFace: "ccw" },
      multisample: { count: MSAA_SAMPLES },
    });

    this.edgePipeline = this.device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: vertexState,
      fragment: {
        module: shader,
        entryPoint: "fs_edge",
        targets: [{ format: this.format }],
      },
      primitive: { topology: "line-list" },
      multisample: { count: MSAA_SAMPLES },
    });

    this.attachInput();
    this.resize();
    new ResizeObserver(() => this.resize()).observe(this.canvas);
    this.writeProj();
    requestAnimationFrame(this.frame);
    return true;
  }

  destroy() {
    this.destroyed = true;
    this.releaseLayerBuffers();
    this.msaaTex?.destroy();
    this.msaaTex = null;
    this.msaaView = null;
  }

  private releaseLayerBuffers() {
    for (const l of this.gpuLayers) {
      l.vbuf.destroy();
      l.tris.destroy();
      l.edges.destroy();
    }
    this.gpuLayers = [];
  }

  loadScene(s: SceneData) {
    this.releaseLayerBuffers();
    for (const ld of s.layers) {
      if (ld.vertices.length === 0) continue;
      const verts = new Float32Array(ld.vertices);
      const tris = new Uint32Array(ld.triangle_indices);
      const edges = new Uint32Array(ld.edge_indices);

      const vbuf = this.device.createBuffer({
        size: verts.byteLength,
        usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
      });
      this.device.queue.writeBuffer(vbuf, 0, verts);

      const tribuf = this.device.createBuffer({
        size: tris.byteLength || 4,
        usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
      });
      if (tris.byteLength > 0) this.device.queue.writeBuffer(tribuf, 0, tris);

      const edgebuf = this.device.createBuffer({
        size: edges.byteLength || 4,
        usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
      });
      if (edges.byteLength > 0) this.device.queue.writeBuffer(edgebuf, 0, edges);

      this.gpuLayers.push({
        layer: ld.layer,
        visible: true,
        vbuf,
        tris: tribuf,
        triCount: tris.length,
        edges: edgebuf,
        edgeCount: edges.length,
      });
    }
    this.cam.bbox = { min: s.bbox_min, max: s.bbox_max };
    this.fitView();
    this.onSceneChanged?.(this.layerInfos(s));
  }

  layers(): number[] {
    return this.gpuLayers.map((l) => l.layer);
  }

  private layerInfos(s: SceneData): LayerInfo[] {
    return s.layers.map((l) => ({
      layer: l.layer,
      color: l.color,
      polygon_count: l.polygon_count,
    }));
  }

  setLayerVisible(layer: number, visible: boolean) {
    const l = this.gpuLayers.find((x) => x.layer === layer);
    if (l) l.visible = visible;
  }

  // -- camera math (mirrors viewport.rs's pre-H1.5 helpers) --

  private aspect(): number {
    const h = this.canvas.height || 1;
    return (this.canvas.width || 1) / h;
  }

  private halfW(): number {
    return this.cam.halfH * this.aspect();
  }

  private pixelToWorld(px: [number, number]): [number, number] {
    const w = this.canvas.width || 1;
    const h = this.canvas.height || 1;
    const nx = (px[0] / w) * 2 - 1;
    const ny = 1 - (px[1] / h) * 2;
    return [
      this.cam.center[0] + nx * this.halfW(),
      this.cam.center[1] + ny * this.cam.halfH,
    ];
  }

  private zoomAt(px: [number, number], factor: number) {
    const before = this.pixelToWorld(px);
    this.cam.halfH = Math.max(1e-3, Math.min(1e12, this.cam.halfH * factor));
    const after = this.pixelToWorld(px);
    this.cam.center[0] += before[0] - after[0];
    this.cam.center[1] += before[1] - after[1];
    this.writeProj();
  }

  private panPixels(dx: number, dy: number) {
    const w = this.canvas.width || 1;
    const h = this.canvas.height || 1;
    const dwx = (dx / w) * 2 * this.halfW();
    const dwy = (dy / h) * 2 * this.cam.halfH;
    this.cam.center[0] -= dwx;
    this.cam.center[1] += dwy;
    this.writeProj();
  }

  fitView() {
    const { min, max } = this.cam.bbox;
    if (min[0] >= max[0] || min[1] >= max[1]) return;
    const cx = (min[0] + max[0]) * 0.5;
    const cy = (min[1] + max[1]) * 0.5;
    const w = max[0] - min[0];
    const h = max[1] - min[1];
    const aspect = Math.max(1e-6, this.aspect());
    const halfH = Math.max(h * 0.5, (w / aspect) * 0.5) * 1.1;
    this.cam.center = [cx, cy];
    this.cam.halfH = Math.max(1, halfH);
    this.writeProj();
  }

  private writeProj() {
    const cx = this.cam.center[0];
    const cy = this.cam.center[1];
    const hh = this.cam.halfH;
    const hw = this.halfW();
    const left = cx - hw, right = cx + hw;
    const bottom = cy - hh, top = cy + hh;
    const rl = right - left;
    const tb = top - bottom;
    const m = new Float32Array([
       2 / rl,            0,                  0, 0,
       0,                 2 / tb,             0, 0,
       0,                 0,                  1, 0,
      -(right + left) / rl, -(top + bottom) / tb, 0, 1,
    ]);
    this.device.queue.writeBuffer(this.uniformBuf, 0, m);
  }

  // -- input --

  private attachInput() {
    const c = this.canvas;
    c.tabIndex = 0;
    c.addEventListener("mousemove", (e) => {
      const r = c.getBoundingClientRect();
      const p: [number, number] = [
        (e.clientX - r.left) * (c.width / r.width),
        (e.clientY - r.top) * (c.height / r.height),
      ];
      if (this.panning) {
        this.panPixels(p[0] - this.panLast[0], p[1] - this.panLast[1]);
      }
      this.cursor = p;
      this.panLast = p;
    });
    c.addEventListener("mousedown", (e) => {
      if (e.button === 1) {
        e.preventDefault();
        this.panning = true;
        this.panLast = this.cursor;
      }
      c.focus();
    });
    c.addEventListener("mouseup", (e) => {
      if (e.button === 1) this.panning = false;
    });
    c.addEventListener("mouseleave", () => { this.panning = false; });
    c.addEventListener("wheel", (e) => {
      e.preventDefault();
      const lines = e.deltaMode === 0 ? e.deltaY / 60 : e.deltaY;
      const factor = Math.exp(lines * 0.15);
      this.zoomAt(this.cursor, factor);
    }, { passive: false });
    c.addEventListener("keydown", (e) => {
      if (e.key === "f" || e.key === "F") {
        this.fitView();
      } else if (e.key === "+" || e.key === "=") {
        this.zoomAt([c.width / 2, c.height / 2], 0.8);
      } else if (e.key === "-" || e.key === "_") {
        this.zoomAt([c.width / 2, c.height / 2], 1.25);
      }
    });
  }

  private resize() {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.floor(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.floor(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.recreateMsaa(w, h);
      this.writeProj();
    } else if (!this.msaaTex) {
      this.recreateMsaa(w, h);
    }
  }

  private recreateMsaa(w: number, h: number) {
    this.msaaTex?.destroy();
    this.msaaTex = this.device.createTexture({
      size: [w, h],
      sampleCount: MSAA_SAMPLES,
      format: this.format,
      usage: GPUTextureUsage.RENDER_ATTACHMENT,
    });
    this.msaaView = this.msaaTex.createView();
    this.diag.msaaW = w;
    this.diag.msaaH = h;
  }

  private pushDiag() {
    this.diag.canvasW = this.canvas.width;
    this.diag.canvasH = this.canvas.height;
    this.diag.layers = this.gpuLayers.length;
    this.onDiag?.({ ...this.diag });
  }

  // -- render loop --

  private frame = () => {
    if (this.destroyed) return;
    try {
      // If MSAA texture size disagrees with canvas size, the resolve
      // target dimension check will fail. Re-sync first.
      if (!this.msaaTex || this.diag.msaaW !== this.canvas.width || this.diag.msaaH !== this.canvas.height) {
        this.recreateMsaa(this.canvas.width || 1, this.canvas.height || 1);
      }
      const tex = this.ctx.getCurrentTexture();
      const resolveView = tex.createView();
      const enc = this.device.createCommandEncoder();
      const pass = enc.beginRenderPass({
        colorAttachments: [
          {
            view: this.msaaView!,
            resolveTarget: resolveView,
            loadOp: "clear",
            storeOp: "store",
            clearValue: { r: 0.06, g: 0.06, b: 0.08, a: 1.0 },
          },
        ],
      });

      pass.setBindGroup(0, this.uniformBg);
      pass.setPipeline(this.fillPipeline);
      for (const l of this.gpuLayers) {
        if (!l.visible || l.triCount === 0) continue;
        pass.setVertexBuffer(0, l.vbuf);
        pass.setIndexBuffer(l.tris, "uint32");
        pass.drawIndexed(l.triCount);
      }
      pass.setPipeline(this.edgePipeline);
      for (const l of this.gpuLayers) {
        if (!l.visible || l.edgeCount === 0) continue;
        pass.setVertexBuffer(0, l.vbuf);
        pass.setIndexBuffer(l.edges, "uint32");
        pass.drawIndexed(l.edgeCount);
      }
      pass.end();
      this.device.queue.submit([enc.finish()]);
      this.diag.frames++;
      if (this.diag.frames % 30 === 1) this.pushDiag();
    } catch (e) {
      this.diag.err = String(e);
      this.pushDiag();
    }
    requestAnimationFrame(this.frame);
  };
}
