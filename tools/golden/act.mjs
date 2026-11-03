// Dumps reference data for the rig / act port:
//   tests/golden/act_poses.json  pose_at / lerp_pose samples over the fireball-throw pose track
//   tests/golden/act_*.bin       acts drawn without any effect (an empty chain): hero, straw target, dust, straw
// .bin layout: u32le width, u32le height, i32le frame origin x, i32le frame origin y, then width*height RGBA bytes
// (composited over #201020).
// Usage: PIXELFX_DIR=<path to a pixelfx clone> node tools/golden/act.mjs
// The poses and test acts below are mirrored in src/fx/act.rs tests (`fireball_track`, `test_act`); keep them in sync.
// Each frame is rendered in a fresh child process: the rig caches renders by a rounded pose key, so which pixels a
// pose gets depends on what was rendered before it in the same process. The Rust side clears its cache per frame too.
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";

const dir = process.env.PIXELFX_DIR;
if (!dir) {
  console.error("PIXELFX_DIR is not set (path to a pixelfx clone)");
  process.exit(1);
}
const core = (f) => import(pathToFileURL(join(resolve(dir), "src/core", f)).href);
const { createFrame, composite } = await core("fx.js");
const { hex } = await core("color.js");
const { READY } = await core("poses.js");
const { poseAt, lerpPose, createAct, actBounds } = await core("act.js");
const { chain } = await core("chain.js");

const BG = hex("#201020");
const out = join(dirname(fileURLToPath(import.meta.url)), "../../tests/golden");
mkdirSync(out, { recursive: true });

// ---- the fireball-throw act (upstream src/fx/moves/fireball-throw.js), target moved to the scene distance
const THROW = 0.95, FLY = 0.46, HIT = THROW + FLY, TX = 70;
const HOLD = { ...READY, x: -3, crouch: 4, lean: 2, fx: 18, fy: -22, bx: 11, by: -17, ffx: 10, fbx: -8 };
const THROWN = { ...READY, x: 9, crouch: 4, lean: 6, fx: 19, fy: -22, bx: -6, by: -24, ffx: 13, fbx: -10, toeB: true };
const FLINCH = { turn: 0.9, x: 7, crouch: 5, lean: -3, fx: -4, fy: -36, bx: 5, by: -33, ffx: 4, fbx: -4 };
const POSES = [
  [-0.42, { ...READY, x: -3 }],
  [0.08, { ...HOLD }],
  [0.5, HOLD],
  [THROW - 0.03, { ...HOLD, crouch: 5, lean: -2, fx: 17, bx: 5, by: -20 }],
  [THROW, THROWN, "snap"],
  [HIT - 0.02, { ...THROWN, crouch: 3, lean: 4 }],
  [HIT, FLINCH, "snap"],
  [HIT + 0.7, { ...FLINCH, crouch: 3, lean: -2 }],
  [HIT + 1.4, { ...READY, x: 4 }],
  [HIT + 2.1, { ...READY, x: -3 }],
  [HIT + 5.4, { ...READY, x: -3 }],
];
const meta = { name: "test", size: [120, 70], origin: [30, 62], duration: HIT + 5.4, loop: false, fps: 30 };
const MODS = {
  ready: { meta: { ...meta, duration: 1 }, act: { target: { x: TX }, poses: [[0, READY]] }, create: chain([]) },
  throw: {
    meta,
    act: {
      target: { x: TX },
      poses: POSES,
      hits: [
        { t: HIT, stop: 0.18, shake: 0.38, amp: 3, push: 3.4, straw: 34, skin: "char" },
        { t: HIT + 4.8, push: 0, straw: 0, flash: 0, skin: null },
      ],
      dust: [{ t: THROW, x: 13, n: 7, dir: 1 }],
    },
    create: chain([]),
  },
};
// [file, act, updates at 1/30 s before the frame is drawn]
const FRAMES = [
  ["act_ready.bin", "ready", 0],
  ["act_throw.bin", "throw", 45],
  ["act_hit.bin", "throw", 66],
];

function renderFrame(name, mod, steps) {
  const m = MODS[mod], b = actBounds(m), w = b.x1 - b.x0, h = b.y1 - b.y0;
  const f = createFrame(w, h, b.x0, b.y0);
  const act = createAct(m, { x: 0, y: 0, seed: 1 });
  for (let i = 0; i < steps; i++) act.update(1 / 30);
  act.draw(f);
  const bytes = composite(f, new Uint8ClampedArray(w * h * 4), BG);
  const head = Buffer.alloc(16);
  head.writeUInt32LE(w, 0);
  head.writeUInt32LE(h, 4);
  head.writeInt32LE(b.x0, 8);
  head.writeInt32LE(b.y0, 12);
  writeFileSync(join(out, name), Buffer.concat([head, Buffer.from(bytes.buffer)]));
  console.log("wrote", name, w, "x", h, "origin", b.x0, b.y0, "t", act.t.toFixed(3), "shake", act.shake.join(","));
}

const [, , flag, ...rest] = process.argv;
if (flag === "--frame") {
  const [name, mod, steps] = rest;
  renderFrame(name, mod, Number(steps));
} else {
  // pose samples
  const samples = [];
  const times = [];
  for (let i = 0; i <= 160; i++) times.push(-0.6 + i * 0.05);
  for (const [t] of POSES) times.push(t, t - 1e-3, t + 1e-3);
  for (const t of times) samples.push({ t, pose: poseAt(POSES, t) });
  const A = { hx: 1, wl: 0.5, toeB: true, weapon: "sword", sa: -0.85, fz: 2 };
  const B = { fx: 3, ghost: 1, armsFront: true, hy: -1, yaw: 0.5 };
  const lerps = [];
  for (const [a, b] of [[HOLD, THROWN], [FLINCH, READY], [A, B], [B, A], [{}, {}]])
    for (const u of [0, 0.3, 0.77, 1]) lerps.push({ a, b, u, pose: lerpPose(a, b, u) });
  writeFileSync(join(out, "act_poses.json"), JSON.stringify({ samples, lerps }));
  console.log("wrote act_poses.json", samples.length, "samples", lerps.length, "lerps");
  const self = fileURLToPath(import.meta.url);
  for (const [name, mod, steps] of FRAMES)
    process.stdout.write(execFileSync(process.execPath, [self, "--frame", name, mod, String(steps)], { env: process.env }));
}
