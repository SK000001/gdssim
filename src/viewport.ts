// WebGPU viewport: render the polygons returned by `load_gds` into a
// <canvas> in the Tauri webview. Replaces the old Rust-owned winit
// window. Rust still does parsing / hierarchy flatten / triangulation;
// this file just owns the GPU buffers + camera + input.

import shaderSrc from "./viewport.wgsl?raw";

export type SceneData = {
  polygon_count: number;
  layers: number[];
  bbox_min: [number, number];
  bbox_max: [number, number];
  vertices: number[]; // interleaved x, y, r, g, b
  indices: number[];
};

type Camera = {
  center: [number, number];
  halfH: number;
  bbox: { min: [number, number]; max: [number, number] };
};

const VERTEX_FLOATS = 5; // x, y, r, g, b

export class Viewport {
  private canvas: HTMLCanvasElement;
  private device!: GPUDevice;
  private ctx!: GPUCanvasContext;
  private format!: GPUTextureFormat;
  private pipeline!: GPURenderPipeline;
  private uniformBuf!: GPUBuffer;
  private uniformBg!: GPUBindGroup;
  private vbuf: GPUBuffer | null = null;
  private ibuf: GPUBuffer | null = null;
  private indexCount = 0;
  private cam: Camera = {
    center: [0, 0],
    halfH: 150,
    bbox: { min: [0, 0], max: [0, 0] },
  };
  private cursor: [number, number] = [0, 0];
  private panning = false;
  private panLast: [number, number] = [0, 0];
  private destroyed = false;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
  }

  /** Returns true on success, false if WebGPU isn't available. */
  async init(): Promise<boolean> {
    if (!("gpu" in navigator)) {
      return false;
    }
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

    this.pipeline = this.device.createRenderPipeline({
      layout: this.device.createPipelineLayout({ bindGroupLayouts: [bgLayout] }),
      vertex: {
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
      },
      fragment: {
        module: shader,
        entryPoint: "fs_main",
        targets: [{ format: this.format }],
      },
      primitive: { topology: "triangle-list", frontFace: "ccw" },
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
  }

  loadScene(s: SceneData) {
    if (s.indices.length === 0) {
      this.vbuf = null;
      this.ibuf = null;
      this.indexCount = 0;
      return;
    }
    const verts = new Float32Array(s.vertices);
    const indices = new Uint32Array(s.indices);

    if (this.vbuf) this.vbuf.destroy();
    if (this.ibuf) this.ibuf.destroy();

    this.vbuf = this.device.createBuffer({
      size: verts.byteLength,
      usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
    });
    this.device.queue.writeBuffer(this.vbuf, 0, verts);

    this.ibuf = this.device.createBuffer({
      size: indices.byteLength,
      usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
    });
    this.device.queue.writeBuffer(this.ibuf, 0, indices);

    this.indexCount = indices.length;
    this.cam.bbox = { min: s.bbox_min, max: s.bbox_max };
    this.fitView();
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
    this.cam.center[1] += dwy; // pixel-y down ↔ world-y up
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
    // Column-major mat4x4<f32> — same layout the Rust helper used.
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
    c.tabIndex = 0; // accept keyboard focus
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
        // Middle button.
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
      this.writeProj();
    }
  }

  // -- render loop --

  private frame = () => {
    if (this.destroyed) return;
    const tex = this.ctx.getCurrentTexture();
    const enc = this.device.createCommandEncoder();
    const pass = enc.beginRenderPass({
      colorAttachments: [
        {
          view: tex.createView(),
          loadOp: "clear",
          storeOp: "store",
          clearValue: { r: 0.06, g: 0.06, b: 0.08, a: 1.0 },
        },
      ],
    });
    if (this.vbuf && this.ibuf && this.indexCount > 0) {
      pass.setPipeline(this.pipeline);
      pass.setBindGroup(0, this.uniformBg);
      pass.setVertexBuffer(0, this.vbuf);
      pass.setIndexBuffer(this.ibuf, "uint32");
      pass.drawIndexed(this.indexCount);
    }
    pass.end();
    this.device.queue.submit([enc.finish()]);
    requestAnimationFrame(this.frame);
  };
}
