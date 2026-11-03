// Dumps reference composites of upstream pixelfx particle simulations into tests/golden/sim_*.bin.
// File layout: u32le width, u32le height, then width*height RGBA bytes.
// Usage: PIXELFX_DIR=<path to a pixelfx clone> node tools/golden/sim.mjs
// The scenes below are mirrored by `scene_particles` / `scene_bits` in src/fx/sim.rs tests; keep them in sync.
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const dir = process.env.PIXELFX_DIR;
if (!dir) {
  console.error("PIXELFX_DIR is not set (path to a pixelfx clone)");
  process.exit(1);
}
const core = (f) => import(pathToFileURL(join(resolve(dir), "src/core", f)).href);
const { createFrame, painterFor, composite } = await core("fx.js");
const { hex } = await core("color.js");
const { Particles, Blazes, Bits, Lights, P, PF, B, sparkle, flare, band, crystal } = await core("sim.js");

const BG = hex("#201020");
const out = join(dirname(fileURLToPath(import.meta.url)), "../../tests/golden");
mkdirSync(out, { recursive: true });
function dump(name, f) {
  const bytes = composite(f, new Uint8ClampedArray(f.w * f.h * 4), BG);
  const head = Buffer.alloc(8);
  head.writeUInt32LE(f.w, 0);
  head.writeUInt32LE(f.h, 4);
  writeFileSync(join(out, name), Buffer.concat([head, Buffer.from(bytes.buffer)]));
  console.log("wrote", name);
}

// Scene A: every particle kind, a brand that lands and starts a blaze, a light.
{
  const blazes = {};
  const ps = new Particles({ seed: 7, ground: () => 20, haze: "#88aaff", onLand: (x, y) => blazes.b.add(x, y, 0.8, 1) });
  blazes.b = new Blazes(ps);
  const lights = new Lights();
  const W = "#ffffff";
  const S = [
    [P.SPARK, -20, -10, 50, -90, 1.2, W, 1, 0, 0],
    [P.SPARK, -18, -8, -40, -60, 1.0, W, 1, 0, 0],
    [P.MOTE, -10, -5, 90, -20, 0.8, "#7fd0ff", 1.2, 0, 0],
    [P.STREAK, -25, 0, 120, 10, 0.9, "#ffd37f", 1, 0, 0],
    [P.FLAKE, 0, -20, 5, 20, 1.5, "#ffffff", 1.6, 0, 0],
    [P.VAPOUR, 10, 10, 0, -10, 1.4, "#a0b0c0", 4, 0, 0],
    [P.DUST, 15, 15, 10, -5, 1.2, "#b08060", 3, 0, 0],
    [P.STAR, 20, -15, 0, 0, 1.5, "#ffe26b", 2.5, 0, 0],
    [P.EMBER, -5, 15, 10, -30, 1.4, "#ff8a2a", 2, 0, 0],
    [P.SMOKE, 22, 5, 0, -8, 1.5, 0, 3, 0, PF.BIG],
    [P.FIRE, -12, 18, 0, -20, 0.5, 0, 3, 0, PF.SMOKES],
    [P.FIRE, -8, 18, 0, -20, 0.6, 0, 2.5, 0.1, 0],
    [P.BRAND, 5, -18, 30, -60, 1.5, W, 1, 0, 0],
    [P.GRIT, 25, -10, -60, -100, 1.2, "#c0a070", 2, 0, 0],
    [P.GRIT, -28, -10, 30, -50, 1.2, "#c0a070", 1, 0, 0],
  ];
  for (const s of S) ps.spawn(...s);
  lights.add(0, 0, 14, "#ff8a2a", 0.6, 1.0);
  for (let i = 0; i < 20; i++) {
    ps.update(1 / 30);
    blazes.b.update(1 / 30);
    lights.update(1 / 30);
  }
  const f = createFrame(64, 48);
  const p = painterFor(f).place(32, 24);
  blazes.b.drawBack(p);
  ps.draw(p, "back");
  ps.draw(p, "front");
  lights.draw(p);
  dump("sim_particles.bin", f);
  console.log("particles alive:", ps.count, "blazes:", blazes.b.count);
}

// Scene B: debris of each kind, then the glint / flare / band / crystal helpers.
{
  const bits = new Bits({ ground: (x) => 16 + x * 0.05 });
  bits.add({ kind: B.SHARD, x: -20, y: -10, vx: 40, vy: -80, a: 0.3, va: 6, s: 5, c: "#ffd37f", bounce: 0.4, life: 2 });
  bits.add({ kind: B.CHUNK, x: 0, y: -12, vx: -20, vy: -60, a: 0.5, va: 3, s: 4, bounce: 0.3, life: 3 });
  bits.add({ kind: B.CHUNK, x: 14, y: -14, vx: 10, vy: -90, a: 1.1, va: -4, s: 3, g: 0.8, drag: 0.5, bounce: 0.5, life: 3 });
  bits.add({ kind: B.DRIP, x: 18, y: -6, c: "#4fa0ff", life: 2 });
  for (let i = 0; i < 20; i++) bits.update(1 / 30);
  const f = createFrame(64, 48);
  const p = painterFor(f).place(32, 24);
  bits.draw(p);
  sparkle(p, -22, -18, 4, hex("#ffe26b"), hex("#ffffff"), 0.5);
  flare(p, 20, -16, 8, 0.6, ["#401010", "#a02020", "#ff6030", "#ffd080", "#fff0c0"]);
  band(p, 0, 5, 14, 0.4, 3, 0.8, ["#401010", "#a02020", "#ff6030", "#ffd080", "#fff0c0"], (g) => Math.sin(g) < 0);
  crystal(p, -12, 20, 16, 8, 2, ["#14182c", "#2a3a6a", "#3d5fa0", "#5f8fd0", "#8fc0f0", "#cfeaff"], { glint: 0.5 });
  dump("sim_bits.bin", f);
  console.log("bits alive:", bits.count);
}
