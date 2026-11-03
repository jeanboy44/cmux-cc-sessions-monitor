// Dumps reference outputs of upstream pixelfx core/math.js into tests/golden/math.json.
// Usage: PIXELFX_DIR=<path to a pixelfx clone> node tools/golden/dump.mjs
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const dir = process.env.PIXELFX_DIR;
if (!dir) {
  console.error("PIXELFX_DIR is not set (path to a pixelfx clone)");
  process.exit(1);
}
const m = await import(pathToFileURL(join(resolve(dir), "src/core/math.js")).href);

const hashInputs = [[], [0], [1], [7, 3], [42, 0, 1], [-5, 99], [123456789, 987654321, 5], [1, 2, 3, 4, 5]];
const hash = hashInputs.map((a) => ({ args: a, hash: m.hash(...a), rand: m.rand(...a) }));

const rng = [7, 42].map((seed) => {
  const r = new m.Rng(seed);
  return { seed, values: Array.from({ length: 20 }, () => r.next()) };
});

const noise = [];
for (const seed of [0, 5]) for (let i = -6; i <= 6; i++) { const x = i * 0.37; noise.push({ x, seed, v: m.noise(x, seed) }); }
const noise2 = [];
for (const seed of [0, 9]) for (let i = -3; i <= 3; i++) for (let j = -3; j <= 3; j++) {
  const x = i * 0.53 + 0.1, y = j * 0.41 - 0.2;
  noise2.push({ x, y, seed, v: m.noise2(x, y, seed) });
}
const fbm = [];
for (const octaves of [1, 3, 5]) for (let i = -2; i <= 2; i++) for (let j = -2; j <= 2; j++) {
  const x = i * 0.77 + 0.3, y = j * 0.61 - 0.4;
  fbm.push({ x, y, seed: 3, octaves, v: m.fbm(x, y, 3, octaves) });
}

const tile = [];
for (const [px, py] of [[0, 0], [4, 0], [0, 3], [5, 3]]) for (let i = -2; i <= 3; i++) for (let j = -2; j <= 3; j++) {
  const x = i * 1.9 + 0.3, y = j * 1.3 - 0.7;
  tile.push({ x, y, px, py, seed: 2, v: m.tileNoise(x, y, px, py, 2), f: m.tileFbm(x, y, px, py, 2, 4) });
}
const loop = [];
for (const radius of [1, 2.5]) for (let i = 0; i <= 8; i++) { const phase = i / 8 + 0.013; loop.push({ phase, radius, seed: 1, v: m.loopNoise(phase, radius, 1) }); }

const out = join(dirname(fileURLToPath(import.meta.url)), "../../tests/golden/math.json");
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify({ hash, rng, noise, noise2, fbm, tile, loop }, null, 1) + "\n");
console.log("wrote", out);
