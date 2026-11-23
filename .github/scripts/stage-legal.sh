#!/usr/bin/env bash
# Stage the legal files inside the Linux tar.gz we ship. Adding bundle
# resources alone would not put them in the archive. RELEASE_VERSION must be
# set by the caller.
set -euo pipefail

: "${RELEASE_VERSION:?RELEASE_VERSION is required}"

pnpm --filter @buddy/desktop exec tauri build --bundles appimage,deb,rpm

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/legal"

# NOTICE requires the MPL text to accompany the distribution. Keep a copy in
# the repo: the MPL crates are not fetched on every target.
cp NOTICE LICENSE LICENSE-MPL-2.0.txt "$stage/legal/"

{
  echo "# MPL-2.0 source locations for Buddy $RELEASE_VERSION"
  echo
  echo "This build contains MPL-2.0 licensed crates and packages. Their complete"
  echo "corresponding source, at the exact versions below, is available from the"
  echo "registry named in each entry."
  echo
  awk '
    $3 == "MPL-2.0" && $2 ~ /^[0-9]/ {
      printf "- %s %s: https://crates.io/api/v1/crates/%s/%s/download\n", $1, $2, $1, $2
      next
    }
    $2 == "MPL-2.0" && $1 ~ /@[0-9]/ {
      spec = $1
      match(spec, /^.*@/)
      name = substr(spec, 1, RLENGTH - 1)
      version = substr(spec, RLENGTH + 1)
      printf "- %s: https://registry.npmjs.org/%s/-/%s-%s.tgz\n", spec, name, name, version
    }
  ' NOTICE
  echo
  echo "cssparser, cssparser-macros and dtoa-short ship the MPL-2.0 text as LICENSE"
  echo "in their crate sources; option-ext ships it as LICENSE.txt. The selectors"
  echo "crates ship no license file, so their upstream notice is kept at"
  echo "https://github.com/servo/servo/blob/main/LICENSE. The MPL-2.0 text is"
  echo "staged beside this file as LICENSE-MPL-2.0.txt."
} > "$stage/legal/MPL-SOURCE-LOCATIONS.md"

tar czf buddy-linux-x86_64.tar.gz \
  -C target/release buddy \
  -C "$stage" legal
