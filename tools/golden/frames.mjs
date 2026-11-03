// Dumps reference composites of upstream pixelfx Painter scenes into tests/golden/painter_*.bin.
// File layout: u32le width, u32le height, then width*height RGBA bytes.
// Usage: PIXELFX_DIR=<path to a pixelfx clone> node tools/golden/frames.mjs
// The scenes below are mirrored by `scene_a` / `scene_b` in src/fx/painter.rs tests; keep them in sync.
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
const { hex, mix } = await core("color.js");
const { star } = await core("shapes.js");

const RED = hex("#e43b44"), YEL = hex("#feae34"), BLU = hex("#0099db"), GRN = hex("#63c74d"), WHT = hex("#ffffff");
const BG = hex("#201020");

function scene(p, withFade) {
  p.dot(-20, -15, WHT);
  p.px(-19, -15, YEL);
  p.line(-24, -18, -4, -10, (u) => mix(RED, YEL, u));
  p.circle(-12, 4, 8, (d) => mix(WHT, BLU, d), (d) => 1 - d * 0.5);
  p.shade(2, -20, 22, -6, (x, y, o) => {
    o.c = mix(GRN, BLU, (x - 2) / 20);
    o.a = (y + 20) / 14;
    o.k = 0.3;
  });
  p.rect(8, 2, 10, 6, RED, 0.6);
  p.ellipse(14, 14, 9, 4, YEL, 1);
  p.ring(-14, 14, 7, 2, BLU, 1);
  p.arc(0, 0, 18, 5, 0.2, 2.4, (u) => mix(YEL, RED, u), 1);
  p.beam(-22, 20, 20, -14, 3, (u) => mix(BLU, WHT, u), 0.8);
  p.poly(star(18, 18, 3, 7), YEL, 1);
  p.tri(-6, -22, 0, -14, -12, -14, GRN, 1);
  p.disc(-4, 10, 6, (d) => mix(WHT, RED, d), (d) => 0.7 * (1 - d));
  p.glow(0, 0, 14, YEL, 0.6);
  p.glowLine(-20, 8, 20, 8, 5, BLU, 0.5);
  p.glowRing(0, 0, 20, 4, GRN, 0.5);
  p.light(5, 5, WHT, 1);
  if (withFade) {
    p.fade = 0.5;
    p.rect(-24, -24, 14, 10, WHT, 1);
    p.lightK = 0.5;
    p.glow(-18, 18, 8, RED, 1);
  }
}

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

const a = createFrame(64, 48);
scene(painterFor(a).place(32, 24), true);
dump("painter_a.bin", a);

const b = createFrame(64, 48, -10, -5);
scene(painterFor(b).place(22, 14, true, 0.6), false);
dump("painter_b.bin", b);
