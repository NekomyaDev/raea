# Release recipes

## Linux AppImage

`release.yml` builds an x86_64 AppImage after the Linux tarball passes its checks,
for both source builds and promoted candidates. It attaches
`raea-VERSION-linux-x86_64.AppImage` and `.AppImage.sha256` beside the existing
archives and attests the AppImage. Tag pushes (`v*`) and manual runs use the same
steps; a manual `dry_run` keeps the files as workflow artifacts without publishing.
The existing skills gate still applies (`skills_passed` or the tag's
`skills-passed:` line when the feed app is absent).

To package an existing, checked Linux release locally on Ubuntu 24.04 x86_64:

```sh
bash tools/release-appimage.sh 0.1.6 dist/raea-0.1.6-linux-x86_64.tar.gz
```

Keep the tarball's `.sha256` beside it. The script verifies it and its version,
preserves the SDK, model, skills and companion hosts, then uses a version and
SHA256 pinned [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy/releases/tag/1-alpha-20251107-1)
to collect linked libraries and package the image. The bundler stays beside the
executables because RAEA loads it at runtime. Project licenses and the distribution's
copyright notices for bundled libraries travel with the image. `curl`, `binutils`,
`coreutils`, `tar` and `dpkg-query` are required, alongside the normal release's
runtime libraries and Xvfb for the checks. `RAEA_LINUXDEPLOY=/path/to/linuxdeploy-x86_64.AppImage`
uses a previously downloaded copy, with the same checksum verification. The AppImage
runtime is also version and checksum pinned; `RAEA_APPIMAGE_RUNTIME=/path/to/runtime-x86_64`
supplies a local copy. With both overrides, the packaging tools need no downloads.
An optional third argument selects the output directory (default: `dist/`).

The AppImage runtime is smoke tested without FUSE, and the full
`tools/release-check.sh` suite runs through the extracted image's `AppRun` before
the output and its checksum are written. `RAEA_CHECK_DIR` keeps failed checks'
frames and logs, as for the tarball.

This packages the public `raea` engine command and its app hosts. The desktop editor
is a separate distribution whose source is absent from this checkout. `AppRun`
preserves the caller's working directory and arguments; it does not install RAEA
or the agent skills into the user's home.

```sh
sha256sum -c raea-0.1.6-linux-x86_64.AppImage.sha256
chmod +x raea-0.1.6-linux-x86_64.AppImage
./raea-0.1.6-linux-x86_64.AppImage --help
./raea-0.1.6-linux-x86_64.AppImage render hello.tsx -o hello.mp4
# Without FUSE (extracts to temporary storage for this invocation):
./raea-0.1.6-linux-x86_64.AppImage --appimage-extract-and-run --help
```

The Ubuntu 24.04 build still requires glibc 2.39 or newer, a Vulkan loader and GPU
driver (or Mesa's software Vulkan), and a display for windowed apps. System
libraries excluded by linuxdeploy, including ALSA and fontconfig, and optional
tools such as FFmpeg are still host requirements, as in the tarball's README.
Install a CJK font for Japanese, Chinese and Korean text. Normal mounted execution
also needs FUSE; [AppImage's fallback options](https://docs.appimage.org/user-guide/troubleshooting/fuse.html)
cover machines without it.

## Earlier release recipes

How 0.1.0-canary.2 and canary.3 were built and checked beyond `tools/release.sh` (kept for the next):

- `linux/Dockerfile`: the Ubuntu 24.04 build root (glibc 2.39), run under Rosetta; bun's x64 baseline build, as the
  default bun segfaults there.
- `linux/build.sh VERSION [JOBS]`: in that container, raea with the plugin catalog's features and the two app hosts,
  strip, then `tools/release.sh` under Xvfb (its header has the pitfalls). The VM: `colima start -p x86 --vm-type vz`,
  then `rosetta: true` in ~/.colima/x86/colima.yaml and `vmOpts.vz.rosetta {enabled, binfmt}` in its lima.yaml, and
  `LIMA_HOME=~/.colima/_lima limactl start colima-x86`; docker at unix://~/.colima/x86/docker.sock. Stop it to free a
  deleted target's disk. It runs `tools/cpu-heartbeat.sh`
  mounted as /hb.sh beside the build, so antsy's --stall sees a working compiler.
- `linux/Dockerfile.trixie` and `linux/rl3.sh`: RL3 on Linux, the tarball on a fresh debian:trixie with a new user and
  no network.
- `rl3-mac.sh TARBALL [PONG_DIR]`: RL3 on the Mac, the tarball as a fresh user would run it, sandboxed.
- `../release-web.sh VERSION REF --upload TAG`: the web modules built from REF (core/shape's for @raea/r3f's loadShapes,
  raea-web's for @raea/web) with their SHA256SUMS, attached to the GitHub release TAG: vVERSION beside the archives
  for every release (canary.1 and canary.2 got theirs by hand on 11-06, built from their own shape sources), and between releases a pre-release web-SHA on the public commit SHA
  (`gh release create web-SHA -R shinyobjectz/raea --target SHA --prerelease` first),
  when core/shape, engine/shape-web or engine/web changed (docs/r3f.feature R3F5).
