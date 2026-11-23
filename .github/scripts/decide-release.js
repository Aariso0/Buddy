// Decide whether the package version warrants a release by comparing it with
// the latest valid v* tag. Prints `version=...` and `should_release=...` so the
// caller can append them to $GITHUB_OUTPUT.
//
// Compare digit strings exactly: Number loses precision above 2^53.
const cmpNum = (a, b) => a.length === b.length
  ? (a < b ? -1 : a > b ? 1 : 0)
  : (a.length < b.length ? -1 : 1);

const parse = (s) => {
  const m = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/.exec(s);
  if (!m) return null;
  const pre = m[4] ? m[4].split(".") : [];
  const build = m[5] ? m[5].split(".") : [];
  // SemVer forbids empty identifiers and leading zeroes in numeric
  // prerelease identifiers; invalid tags must not suppress a release.
  if (build.some((id) => !id)) return null;
  if (pre.some((id) => !id || (/^\d+$/.test(id) && id.length > 1 && id[0] === "0"))) return null;
  return { n: [m[1], m[2], m[3]], p: pre };
};

const cmp = (a, b) => {
  for (let i = 0; i < 3; i++) {
    const order = cmpNum(a.n[i], b.n[i]);
    if (order) return order;
  }
  if ((a.p.length === 0) !== (b.p.length === 0)) return a.p.length ? -1 : 1;
  for (let i = 0; i < Math.min(a.p.length, b.p.length); i++) {
    const x = a.p[i], y = b.p[i], xn = /^\d+$/.test(x), yn = /^\d+$/.test(y);
    if (xn && yn) { const order = cmpNum(x, y); if (order) return order; }
    else if (xn !== yn) return xn ? -1 : 1;
    else if (x !== y) return x < y ? -1 : 1;
  }
  return Math.sign(a.p.length - b.p.length);
};

const version = parse(process.env.VERSION);
if (!version) throw new Error(`invalid package version: ${process.env.VERSION}`);
const entries = process.env.TAGS.split("\n")
  .map((tag) => ({ tag: tag.trim(), value: parse(tag.trim()) }))
  .filter((entry) => entry.value);
let latest = null;
for (const entry of entries) {
  if (!latest || cmp(entry.value, latest.value) > 0) latest = entry;
}
console.log(`version=${process.env.VERSION}`);
console.log(`should_release=${!latest || cmp(version, latest.value) > 0}`);
console.log(`latest_tag=${latest ? latest.tag : "none"}`);
