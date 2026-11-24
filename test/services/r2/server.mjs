// Local Cloudflare R2 for the r2:// test tier, simulated by Miniflare (the
// workerd-based runtime behind `wrangler dev`).
//
// Miniflare serves a bucket over R2's S3-compatible API at
// /cdn-cgi/local/r2/s3/<bucket> once the bucket has `s3Credentials`, with R2's
// own rules: equal-size multipart parts, R2's header screening and error codes.
//
// That endpoint only answers requests whose Host is localhost/127.0.0.1, so a
// sidecar container cannot seed the bucket over S3. The fixtures are written
// here through the Workers binding instead (still not by opendal), and the
// healthcheck waits for READY_FILE.
//
// Env (all have test defaults):
//   R2_PORT                   port to listen on
//   R2_BUCKETS                space-separated bucket names
//   R2_ACCESS_KEY_ID          SigV4 access key accepted by the S3 endpoint
//   R2_SECRET_ACCESS_KEY      SigV4 secret key
//   R2_FIXTURES_DIR           files uploaded to <first bucket>/external/
//   R2_READY_FILE             written once the buckets are seeded

import { existsSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Miniflare } from "miniflare";

const port = Number(process.env.R2_PORT ?? "8787");
const buckets = (process.env.R2_BUCKETS ?? "warehouse").split(/\s+/).filter(Boolean);
const s3Credentials = {
  accessKeyId: process.env.R2_ACCESS_KEY_ID ?? "r2-test-access-key",
  secretAccessKey: process.env.R2_SECRET_ACCESS_KEY ?? "r2-test-secret-key",
};
const fixturesDir = process.env.R2_FIXTURES_DIR ?? "/fixtures";
const readyFile = process.env.R2_READY_FILE ?? "/tmp/r2-ready";

const binding = (bucket) => `BUCKET_${bucket.toUpperCase().replace(/[^A-Z0-9]/g, "_")}`;

rmSync(readyFile, { force: true });

const mf = new Miniflare({
  modules: true,
  script: `export default { fetch() { return new Response("opendal r2 test tier"); } };`,
  // Matches the pinned workerd release (package.json).
  compatibilityDate: "2026-07-30",
  host: "0.0.0.0",
  port,
  // Do not fetch Request.cf data from Cloudflare at startup.
  cf: false,
  r2Buckets: Object.fromEntries(buckets.map((id) => [binding(id), { id, s3Credentials }])),
});
await mf.ready;

const shutdown = async () => {
  await mf.dispose();
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

let seeded = 0;
if (buckets.length > 0 && existsSync(fixturesDir)) {
  const bucket = await mf.getR2Bucket(binding(buckets[0]));
  for (const name of readdirSync(fixturesDir).sort()) {
    const path = join(fixturesDir, name);
    if (!statSync(path).isFile()) continue;
    await bucket.put(`external/${name}`, readFileSync(path));
    seeded += 1;
  }
}

writeFileSync(readyFile, "ready\n");
console.log(
  `r2: serving ${buckets.join(", ")} on :${port}/cdn-cgi/local/r2/s3 ` +
    `(${seeded} fixtures under ${buckets[0]}/external/)`,
);
