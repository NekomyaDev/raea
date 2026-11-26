#!/usr/bin/env bash
# Package the Linux archive produced and checked by tools/release.sh; no Rust rebuild.
#   bash tools/release-appimage.sh VERSION ARCHIVE [OUT_DIR]
# Build on Ubuntu 24.04 x86_64, as release.yml does. Nothing is published here.
set -euo pipefail
die() { echo "appimage: $*" >&2; exit 1; }
version=${1:?usage: release-appimage.sh VERSION ARCHIVE [OUT_DIR]}
archive=${2:?usage: release-appimage.sh VERSION ARCHIVE [OUT_DIR]}
root=$(cd "$(dirname "$0")/.." && pwd)
out=${3:-$root/dist}
[[ $version =~ ^[0-9][0-9A-Za-z.+-]*$ ]] || die "invalid version: $version"
[[ $(uname -s) = Linux && $(uname -m) = x86_64 ]] || die "Linux x86_64 is required"
for tool in curl sha256sum tar readelf dpkg-query; do
  command -v "$tool" >/dev/null || die "missing tool: $tool"
done
archive=$(realpath "$archive")
[[ -f $archive && -f $archive.sha256 ]] || die "need the Linux archive and its .sha256"
[[ $(basename "$archive") = raea-$version-linux-x86_64.tar.gz ]] || die "archive name does not match version/platform"
(cd "$(dirname "$archive")" && sha256sum -c "$(basename "$archive").sha256")
mkdir -p "$out"
out=$(cd "$out" && pwd)
name=raea-$version-linux-x86_64.AppImage
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/raea.AppDir
mkdir -p "$appdir/usr"
tar -xzf "$archive" -C "$work"
for binary in raea raea-perceive raea-rune-host raea-page-host libraea_bundle.so; do
  [[ -f $work/raea/bin/$binary ]] || die "archive is missing bin/$binary"
  readelf -h "$work/raea/bin/$binary" | grep -q 'Machine:.*Advanced Micro Devices X86-64' \
    || die "$binary is not an x86_64 ELF file"
done
[[ $(cat "$work/raea/share/raea/VERSION") = "$version" ]] || die "archive's VERSION does not match $version"
mv "$work/raea/bin" "$work/raea/share" "$appdir/usr/"
mkdir -p "$appdir/usr/share/doc/raea"
cp "$root/LICENSE" "$root/NOTICE" "$appdir/usr/share/doc/raea/"
cp -R "$root/LICENSES" "$appdir/usr/share/doc/raea/"
cp "$work/raea/README" "$appdir/usr/share/doc/raea/README-linux"

# The bundler is opened with dlopen beside raea, so keep it in usr/bin and ask
# linuxdeploy to process its dependencies there, rather than move it to usr/lib.
# A pinned linuxdeploy also carries its AppImage output plugin and appimagetool.
deploy_version=1-alpha-20251107-1
deploy_sha=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
# An existing download can be supplied for offline packaging; it is still verified.
deploy=${RAEA_LINUXDEPLOY:-$work/linuxdeploy-x86_64.AppImage}
if [[ -z ${RAEA_LINUXDEPLOY:-} ]]; then
  curl -fLsS --retry 3 "https://github.com/linuxdeploy/linuxdeploy/releases/download/$deploy_version/linuxdeploy-x86_64.AppImage" -o "$deploy"
fi
deploy=$(realpath "$deploy")
echo "$deploy_sha  $deploy" | sha256sum -c -
chmod +x "$deploy"
# appimagetool otherwise downloads a mutable runtime from the continuous release.
runtime_version=20251108
runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
runtime=${RAEA_APPIMAGE_RUNTIME:-$work/runtime-x86_64}
if [[ -z ${RAEA_APPIMAGE_RUNTIME:-} ]]; then
  curl -fLsS --retry 3 "https://github.com/AppImage/type2-runtime/releases/download/$runtime_version/runtime-x86_64" -o "$runtime"
fi
runtime=$(realpath "$runtime")
echo "$runtime_sha  $runtime" | sha256sum -c -
# Extract the packaging tool once: building needs no FUSE or privileged mount.
(cd "$work" && "$deploy" --appimage-extract >/dev/null)
deploy=$work/squashfs-root/AppRun
export ARCH=x86_64 LINUXDEPLOY_OUTPUT_VERSION="$version" NO_STRIP=1 APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_RUNTIME_FILE="$runtime"
"$deploy" --appdir "$appdir" --deploy-deps-only "$appdir/usr/bin" \
  --desktop-file "$root/tools/release/appimage/raea.desktop" \
  --icon-file "$root/docs/logo/cat-black.svg" --icon-filename raea \
  --custom-apprun "$root/tools/release/appimage/AppRun"

# Keep Debian/Ubuntu's copyright and source notices beside the bundled libraries.
mkdir -p "$appdir/usr/share/doc/raea/bundled-libraries"
while IFS= read -r lib; do
  packages=$(dpkg-query -S "*/$(basename "$lib")" | sed 's/: \/.*//' | sort -u)
  [[ -n $packages ]] || die "no distribution copyright record for $lib"
  while IFS= read -r package; do
    copyright=/usr/share/doc/${package%%:*}/copyright
    [[ -f $copyright ]] || die "no copyright notice for $package"
    cp -L "$copyright" "$appdir/usr/share/doc/raea/bundled-libraries/$package.copyright"
  done <<< "$packages"
done < <(find "$appdir/usr/lib" -type f -name '*.so*')

LDAI_OUTPUT="$work/$name" "$deploy" --appdir "$appdir" --output appimage
[[ -s $work/$name ]] || die "linuxdeploy produced no AppImage"
chmod +x "$work/$name"
# Exercise the outer runtime without FUSE, then run the archive's full release
# checks against the extracted payload through AppRun (SDK, bundler and hosts too).
RAEA_NO_TREE=1 "$work/$name" --appimage-extract-and-run --version
bash "$root/tools/release-check.sh" "$work/$name"
mv "$work/$name" "$out/$name"
(cd "$out" && sha256sum "$name" > "$name.sha256")
echo "appimage: $out/$name"
