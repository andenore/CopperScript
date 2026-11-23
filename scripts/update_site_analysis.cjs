#!/usr/bin/env node
/* Write the promo site's simulation and EMI figures from recorded runs:
   site/assets/simulation-noise.json from a `copper sim` run of the preamp's
   MeasurementNoise bench, and site/assets/emi/*.svg, byte-for-byte copies of
   the `copper emi-check --plot-summary` pages of examples/emi_buck and
   examples/emi_buck_filter, recorded with their SHA-256 in
   site/assets/emi/emi-screening.json. Nothing is simulated here: the curves
   and measures are copied from sim-results.json and the bench's CSV, rounded
   for size.

   node scripts/update_site_analysis.cjs \
     --sim-dir build/promo/sim --emi-bare build/promo/emi/summary.svg \
     --emi-filter build/promo/emi/filter-summary.svg

   See site/README.md for the commands that produce the inputs. */
"use strict";
const crypto = require("node:crypto"), fs = require("node:fs"), path = require("node:path");
const {execFileSync} = require("node:child_process");

const ROOT = path.resolve(__dirname, "..");
const SITE_ASSETS = path.join(ROOT, "site/assets");
const USAGE = "usage: update_site_analysis.cjs --sim-dir DIR --emi-bare SVG --emi-filter SVG";
const BENCH = "MeasurementNoise";

function options(argv) {
  const known = ["--sim-dir", "--emi-bare", "--emi-filter"], result = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!known.includes(argv[i]) || argv[i + 1] === undefined) throw new Error(USAGE);
    result[argv[i]] = argv[i + 1];
  }
  for (const required of known) if (!result[required]) throw new Error(USAGE);
  return result;
}

const sha256 = file => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const significant = value => Number(value.toPrecision(4));
const compiler = () => {
  try { return execFileSync("git", ["rev-parse", "--short", "HEAD"], {cwd: ROOT, encoding: "utf8"}).trim(); }
  catch { return null; }
};

function simulation(dir) {
  const resultsFile = path.join(dir, "sim-results.json"), csvFile = path.join(dir, `${BENCH}.csv`);
  const results = JSON.parse(fs.readFileSync(resultsFile, "utf8"));
  if (results.schema !== "copperscript-sim/v1") throw new Error(`${resultsFile}: not copperscript-sim/v1`);
  const bench = results.benches.find(b => b.name === BENCH);
  if (!bench || bench.status !== "pass") throw new Error(`${resultsFile}: ${BENCH} missing or not passing`);
  const [header, ...rows] = fs.readFileSync(csvFile, "utf8").trim().split(/\r?\n/).map(line => line.split(","));
  const column = name => header.findIndex(h => h.startsWith(name));
  const series = {total: "noise()", sensor: "noise(J_IN)", r_g: "noise(R_G)", amplifier: "noise(U1)", input: "input_noise()"};
  const curve = rows.map(row => [significant(Number(row[column("frequency_hz")])),
    ...Object.values(series).map(name => significant(Number(row[column(name)])))]);
  const measures = Object.fromEntries(bench.measures.map(m => [m.name, {
    expression: m.expression, value: m.steps[0].value, units: m.units, limits: m.limits, status: m.status}]));
  return {
    schema: "copperscript-site-simulation/v1",
    example: "examples/simulation/preamp/board.copper",
    bench_file: "examples/simulation/preamp/benches/noise.copper",
    bench: BENCH,
    command: `copper sim examples/simulation/preamp/board.copper --bench ${BENCH}`,
    compiler_commit: compiler(),
    solver: results.solver,
    source: results.source,
    status: bench.status,
    temperature_k: bench.analysis.temperature_k ?? null,
    series: ["frequency_hz", ...Object.keys(series)],
    units: "V/rtHz",
    curve,
    measures,
    inputs_sha256: {"sim-results.json": sha256(resultsFile), [`${BENCH}.csv`]: sha256(csvFile)}
  };
}

function emi(bare, filter) {
  const dir = path.join(SITE_ASSETS, "emi");
  fs.mkdirSync(dir, {recursive: true});
  const pages = {
    "emi-buck.svg": {from: bare, example: "examples/emi_buck/board.copper"},
    "emi-buck-filter.svg": {from: filter, example: "examples/emi_buck_filter/board.copper"}
  };
  const record = {schema: "copperscript-site-emi/v1", compiler_commit: compiler(), pages: {}};
  for (const [name, page] of Object.entries(pages)) {
    if (!/<title>[^<]*EMI summary/.test(fs.readFileSync(page.from, "utf8").slice(0, 600)))
      throw new Error(`${page.from}: not a copper emi-check summary`);
    fs.copyFileSync(page.from, path.join(dir, name));
    record.pages[name] = {
      example: page.example,
      command: `copper emi-check ${page.example} --allow-proxy-footprints --plot-summary ${name}`,
      example_sha256: sha256(path.join(ROOT, page.example)),
      sha256: sha256(path.join(dir, name))
    };
  }
  fs.writeFileSync(path.join(dir, "emi-screening.json"), JSON.stringify(record, null, 2) + "\n");
}

try {
  const o = options(process.argv.slice(2));
  fs.writeFileSync(path.join(SITE_ASSETS, "simulation-noise.json"), JSON.stringify(simulation(o["--sim-dir"])) + "\n");
  emi(o["--emi-bare"], o["--emi-filter"]);
  console.log("Wrote site/assets/simulation-noise.json and site/assets/emi/.");
} catch (error) {
  console.error(error.message);
  process.exit(2);
}
