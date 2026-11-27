#!/usr/bin/env bash
# The Link window .deb from a build.sh stage; Depends come from dpkg-shlibdeps on the target release.
#   package-deb.sh STAGE OUT_DIR VERSION DISTRO (ubuntu|debian: one file name per build target)
set -euo pipefail
stage=$(realpath "$1") out=$2 version=$3 distro=$4
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
P=$work/andromeda-link-window
cp -a "$stage" "$P"
mkdir -p "$work/debian" "$P/DEBIAN"
echo 'Source: andromeda-link-window' > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O -e"$P/usr/lib/andromeda-link/andromeda-link-app" 2> /dev/null | sed -n 's/^shlibs:Depends=//p')
cat > "$P/DEBIAN/control" << CONTROL
Package: andromeda-link-window
Version: $version
Architecture: amd64
Maintainer: Andromeda <sitish@akintechnologies.com>
Depends: $shlibs, andromeda-link (= $version)
Provides: andromeda-link-app
Conflicts: andromeda-link-app
Replaces: andromeda-link-app
Description: Andromeda Link window
 Pair the phone, mirror its screen, and use its files, texts and calls. It replaces the andromeda-link-app of
 older Andromeda libdesk releases.
CONTROL
mkdir -p "$out"
dpkg-deb --root-owner-group -Zxz -b "$P" "$out/andromeda-link-window-${distro}_${version}_amd64.deb"
