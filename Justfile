build:
  cargo build --release

test:
  cargo nextest run

check:
  cargo check

clippy:
  cargo clippy

fmt:
  cargo fmt

audit:
  cargo audit

publish:
  cargo publish

aur_repo := "ssh://aur@aur.archlinux.org/unpalm.git"

aur:
  #!/usr/bin/env bash
  set -euo pipefail
  source aur/PKGBUILD
  # PKGBUILD downloads the tagged tarball, so the tag must be public first
  git ls-remote --exit-code --tags origin "v$pkgver" > /dev/null \
    || { echo "tag v$pkgver is not on origin, push it first" >&2; exit 1; }
  dir=$(mktemp -d)
  trap 'rm -rf "$dir"' EXIT
  git clone -q "{{aur_repo}}" "$dir"
  cp aur/PKGBUILD "$dir/"
  cd "$dir"
  makepkg --printsrcinfo > .SRCINFO
  git add PKGBUILD .SRCINFO
  if git diff --cached --quiet; then
    echo "AUR package already up to date"
    exit 0
  fi
  git commit -q -m "update to $pkgver-$pkgrel"
  git push -q origin HEAD:master
  echo "AUR package updated to $pkgver-$pkgrel"
