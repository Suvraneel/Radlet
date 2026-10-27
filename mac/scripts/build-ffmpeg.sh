#!/usr/bin/env bash
# Builds the FFmpeg and ffprobe the app downloads when someone picks a
# conversion that needs FFmpeg on a Mac without Homebrew (see
# .github/workflows/ffmpeg.yml, which runs this for both chips).
#
#   mac/scripts/build-ffmpeg.sh arm64|x86_64 [output-folder]
#
# On a Mac either architecture builds on Apple silicon (x86_64 is cross-
# compiled); on Linux, only the machine's own, for trying the script out.
# Output: <output-folder>/ffmpeg and ffprobe (default target/ffmpeg/<arch>),
# self-contained apart from the system's own libraries.
#
# Needs a C and C++ compiler, make, cmake, meson, ninja and pkg-config, and
# nasm for x86_64 (brew install meson ninja nasm pkgconf). Every source is
# pinned by version and SHA-256 and kept under target/ffmpeg-build/src.
#
# The build is LGPL (v2.1 or later): no --enable-gpl, no --enable-nonfree,
# and nothing found automatically. It has what Radlet asks FFmpeg for:
# MP3 (LAME), Opus, Vorbis, VP8/VP9 (libvpx), AV1 decoding (dav1d),
# subtitles drawn with libass (FreeType, FriBidi, HarfBuzz), and on a Mac
# H.264, HEVC and AAC through VideoToolbox and AudioToolbox.
# Running it again carries on where it stopped. TEST_SKIP="lame" leaves
# LAME out, for trying the script where its host isn't reachable.
set -euo pipefail

ARCH="${1:-}"
if [[ "$ARCH" != arm64 && "$ARCH" != x86_64 ]]; then
  echo "usage: build-ffmpeg.sh arm64|x86_64 [output-folder]" >&2
  exit 2
fi
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${2:-$ROOT/target/ffmpeg/$ARCH}"
SRC="$ROOT/target/ffmpeg-build/src"
WORK="$ROOT/target/ffmpeg-build/$ARCH"
PREFIX="$WORK/prefix"
OS="$(uname -s)"
HOST_ARCH="$(uname -m)"
[[ "$HOST_ARCH" == aarch64 ]] && HOST_ARCH=arm64
MIN_MACOS=14.0
JOBS="$(getconf _NPROCESSORS_ONLN 2> /dev/null || echo 4)"
SKIP=" ${TEST_SKIP:-} "

if [[ "$OS" != Darwin && "$ARCH" != "$HOST_ARCH" ]]; then
  echo "On Linux this builds only for this machine ($HOST_ARCH)." >&2
  exit 2
fi
CROSS=0
[[ "$ARCH" != "$HOST_ARCH" ]] && CROSS=1

# name|url|sha256
SOURCES=(
  "ffmpeg|https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n8.0.1.tar.gz|679aa13a19415d5ddab91e580084e3ab20c963c8240001e5cbb955a97bdd81b1"
  "lame|https://downloads.sourceforge.net/project/lame/lame/3.100/lame-3.100.tar.gz|ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e"
  "opus|https://github.com/xiph/opus/archive/refs/tags/v1.5.2.tar.gz|9480e329e989f70d69886ded470c7f8cfe6c0667cc4196d4837ac9e668fb7404"
  "ogg|https://github.com/xiph/ogg/archive/refs/tags/v1.3.6.tar.gz|95b643da661155d79db9de2fca55daed3a8d491039829def246aacb3d9201c81"
  "vorbis|https://github.com/xiph/vorbis/archive/refs/tags/v1.3.7.tar.gz|270c76933d0934e42c5ee0a54a36280e2d87af1de3cc3e584806357e237afd13"
  "libvpx|https://github.com/webmproject/libvpx/archive/refs/tags/v1.15.2.tar.gz|26fcd3db88045dee380e581862a6ef106f49b74b6396ee95c2993a260b4636aa"
  "dav1d|https://github.com/videolan/dav1d/archive/refs/tags/1.5.2.tar.gz|2fc0810b4cdf72784b3c107827ff10b1d83ec709a1ec1fbdbc6a932daf65ead6"
  "freetype|https://github.com/freetype/freetype/archive/refs/tags/VER-2-14-1.tar.gz|44bd69d1f0750603410cfd4f26f1c5523c5a3a087a2dfa8639b757dc7c6677be"
  "fribidi|https://github.com/fribidi/fribidi/releases/download/v1.0.16/fribidi-1.0.16.tar.xz|1b1cde5b235d40479e91be2f0e88a309e3214c8ab470ec8a2744d82a5a9ea05c"
  "harfbuzz|https://github.com/harfbuzz/harfbuzz/releases/download/11.5.1/harfbuzz-11.5.1.tar.xz|972a60a8d274d49e70361da6920c3a73dfb0fb4387f6c6811906a47ba634d8a1"
  "libass|https://github.com/libass/libass/releases/download/0.17.4/libass-0.17.4.tar.xz|78f1179b838d025e9c26e8fef33f8092f65611444ffa1bfc0cfac6a33511a05a"
)

sha256() {
  if command -v sha256sum > /dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# The pinned URL and SHA-256 of a source.
source_of() {
  local entry
  for entry in "${SOURCES[@]}"; do
    if [[ "${entry%%|*}" == "$1" ]]; then
      cut -d'|' -f2,3 <<< "$entry"
      return
    fi
  done
  echo "No source named $1." >&2
  exit 1
}

# Downloads (once) and checks a source, unpacks it into $WORK/<name> (kept
# between runs), and leaves that as the current directory.
unpack() {
  local name="$1" pinned url sum file
  pinned="$(source_of "$name")"
  url="${pinned%%|*}"
  sum="${pinned##*|}"
  file="$SRC/$name-${url##*/}"
  if [[ ! -f "$file" ]]; then
    echo "Downloading ${name}…"
    curl -fsSL --retry 3 -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  if [[ "$(cat "$WORK/$name/.radlet-source" 2> /dev/null)" != "$sum" ]]; then
    if [[ "$(sha256 "$file")" != "$sum" ]]; then
      echo "$file doesn't match its pinned SHA-256; delete it to download it again." >&2
      exit 1
    fi
    rm -rf "${WORK:?}/$name"
    mkdir -p "$WORK/$name"
    tar -xf "$file" -C "$WORK/$name" --strip-components=1
    echo "$sum" > "$WORK/$name/.radlet-source"
  fi
  cd "$WORK/$name"
}

# Builds a source with build_<name> unless it's built already, so a run
# that stopped carries on where it was.
build() {
  local name="$1" sum
  sum="$(source_of "$name")"
  sum="${sum##*|}"
  if [[ "$(cat "$WORK/$name.built" 2> /dev/null)" == "$sum" ]]; then
    return
  fi
  unpack "$name"
  echo "Building ${name} for ${ARCH}…"
  "build_$name"
  echo "$sum" > "$WORK/$name.built"
  cd "$ROOT"
}

# Runs a configure step once per unpacked source.
once() {
  if [[ ! -f .radlet-configured ]]; then
    "$@"
    touch .radlet-configured
  fi
}

mkdir -p "$SRC" "$PREFIX/lib/pkgconfig" "$OUT"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"

if [[ "$OS" == Darwin ]]; then
  TRIPLE="$([[ "$ARCH" == arm64 ]] && echo aarch64-apple-darwin || echo x86_64-apple-darwin)"
  export CC="clang -arch $ARCH" CXX="clang++ -arch $ARCH"
  export CFLAGS="-O2 -mmacosx-version-min=$MIN_MACOS"
  export CXXFLAGS="$CFLAGS" LDFLAGS="-mmacosx-version-min=$MIN_MACOS"
  EXTRA_LIBS=-lc++
  CMAKE_PLATFORM=(-DCMAKE_OSX_ARCHITECTURES="$ARCH" -DCMAKE_OSX_DEPLOYMENT_TARGET="$MIN_MACOS")
  # So projects that look at the processor (Opus) pick the target's code.
  [[ "$CROSS" == 1 ]] && CMAKE_PLATFORM+=(-DCMAKE_SYSTEM_NAME=Darwin -DCMAKE_SYSTEM_PROCESSOR="$ARCH")
else
  TRIPLE="$(cc -dumpmachine)"
  export CC=cc CXX=c++ CFLAGS="-O2 -fPIC" CXXFLAGS="-O2 -fPIC" LDFLAGS=""
  # libvorbis's CMake package leaves out the maths library Linux needs.
  EXTRA_LIBS="-lstdc++ -lm"
  CMAKE_PLATFORM=()
fi

configure_flags=(--prefix="$PREFIX" --disable-shared --enable-static)
[[ "$CROSS" == 1 ]] && configure_flags+=(--host="$TRIPLE")

cmake_build() {
  once cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DCMAKE_PREFIX_PATH="$PREFIX" -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_POLICY_VERSION_MINIMUM=3.5 ${CMAKE_PLATFORM[@]+"${CMAKE_PLATFORM[@]}"} "$@"
  cmake --build build -j "$JOBS"
  cmake --install build
}

meson_build() {
  local cross=()
  if [[ "$CROSS" == 1 ]]; then
    cat > "$WORK/cross.ini" << EOF
[binaries]
c = ['clang', '-arch', '$ARCH']
cpp = ['clang++', '-arch', '$ARCH']
objc = ['clang', '-arch', '$ARCH']
ar = 'ar'
strip = 'strip'
pkg-config = 'pkg-config'
nasm = 'nasm'

[host_machine]
system = 'darwin'
cpu_family = '$([[ "$ARCH" == arm64 ]] && echo aarch64 || echo x86_64)'
cpu = '$ARCH'
endian = 'little'

[built-in options]
c_args = ['-mmacosx-version-min=$MIN_MACOS']
cpp_args = ['-mmacosx-version-min=$MIN_MACOS']
c_link_args = ['-mmacosx-version-min=$MIN_MACOS']
cpp_link_args = ['-mmacosx-version-min=$MIN_MACOS']
EOF
    cross=(--cross-file "$WORK/cross.ini")
  fi
  once meson setup build --prefix="$PREFIX" --libdir=lib --buildtype=release --default-library=static \
    -Db_pie=true ${cross[@]+"${cross[@]}"} "$@"
  meson compile -C build -j "$JOBS"
  meson install -C build
}

build_lame() {
  once ./configure "${configure_flags[@]}" --disable-frontend --disable-decoder --disable-gtktest
  make -j "$JOBS" && make install
}

build_opus() {
  cmake_build -DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF -DOPUS_INSTALL_PKG_CONFIG_MODULE=ON
}

build_ogg() {
  cmake_build -DINSTALL_DOCS=OFF -DBUILD_TESTING=OFF
}

build_vorbis() {
  cmake_build -DBUILD_TESTING=OFF
}

build_libvpx() {
  local target extra=""
  if [[ "$OS" == Darwin ]]; then
    target="$ARCH-darwin20-gcc"
    extra="-mmacosx-version-min=$MIN_MACOS"
  else
    target="$([[ "$ARCH" == arm64 ]] && echo arm64-linux-gcc || echo x86_64-linux-gcc)"
  fi
  # libvpx picks its own compiler and flags for the target.
  once env -u CC -u CXX -u CFLAGS -u CXXFLAGS -u LDFLAGS ./configure --prefix="$PREFIX" \
    --target="$target" --disable-shared --enable-static --enable-pic --disable-examples \
    --disable-tools --disable-docs --disable-unit-tests --enable-vp9-highbitdepth \
    --extra-cflags="$extra"
  make -j "$JOBS" && make install
}

build_dav1d() {
  meson_build -Denable_tools=false -Denable_tests=false -Denable_examples=false
}

build_freetype() {
  meson_build -Dharfbuzz=disabled -Dbrotli=disabled -Dbzip2=disabled -Dpng=disabled -Dzlib=disabled \
    -Dtests=disabled
}

build_fribidi() {
  meson_build -Ddocs=false -Dbin=false -Dtests=false
}

build_harfbuzz() {
  meson_build -Dglib=disabled -Dgobject=disabled -Dcairo=disabled -Dchafa=disabled -Dicu=disabled \
    -Dgraphite2=disabled -Dfreetype=enabled -Dtests=disabled -Ddocs=disabled -Dutilities=disabled \
    -Dintrospection=disabled
}

build_libass() {
  local flags=(--disable-fontconfig --disable-require-system-font-provider --disable-libunibreak)
  [[ "$OS" == Darwin ]] && flags+=(--enable-coretext)
  if [[ "$ARCH" == x86_64 ]] && ! command -v nasm > /dev/null; then
    flags+=(--disable-asm)
  fi
  once ./configure "${configure_flags[@]}" "${flags[@]}"
  make -j "$JOBS" && make install
}

build_ffmpeg() {
  local flags=(
    --prefix="$WORK/ffmpeg-install"
    --pkg-config-flags=--static
    --extra-cflags="$CFLAGS -I$PREFIX/include"
    --extra-ldflags="$LDFLAGS -L$PREFIX/lib"
    --extra-libs="$EXTRA_LIBS"
    --disable-shared --enable-static
    --disable-autodetect --disable-network
    --disable-debug --disable-doc --disable-ffplay
    --enable-zlib
    --enable-libopus --enable-libvorbis --enable-libvpx --enable-libdav1d
    --enable-libass --enable-libfreetype --enable-libfribidi --enable-libharfbuzz
  )
  [[ "$SKIP" != *" lame "* ]] && flags+=(--enable-libmp3lame)
  if [[ "$OS" == Darwin ]]; then
    flags+=(--target-os=darwin --arch="$([[ "$ARCH" == arm64 ]] && echo aarch64 || echo x86_64)"
      --cc="$CC" --cxx="$CXX" --enable-videotoolbox --enable-audiotoolbox)
    [[ "$CROSS" == 1 ]] && flags+=(--enable-cross-compile)
  fi
  once ./configure "${flags[@]}"
  make -j "$JOBS" && make install
}

[[ "$SKIP" != *" lame "* ]] && build lame
for name in opus ogg vorbis libvpx dav1d freetype fribidi harfbuzz libass ffmpeg; do
  build "$name"
done

for tool in ffmpeg ffprobe; do
  cp "$WORK/ffmpeg-install/bin/$tool" "$OUT/$tool"
  strip "$OUT/$tool" 2> /dev/null || true
done

# Every source's licence, to ship beside the programs.
rm -rf "$OUT/licenses"
for name in ffmpeg lame opus ogg vorbis libvpx dav1d freetype fribidi harfbuzz libass; do
  [[ -d "$WORK/$name" ]] || continue
  mkdir -p "$OUT/licenses/$name"
  find "$WORK/$name" -maxdepth 2 -type f \( -iname 'copying*' -o -iname 'license*' -o -iname 'licence*' \
    -o -iname 'ftl.txt' -o -iname 'patents*' \) -exec cp {} "$OUT/licenses/$name/" \;
done

# What the app relies on, checked on the programs themselves (an Intel
# build through Rosetta on Apple silicon).
runner=()
can_run=1
if [[ "$CROSS" == 1 ]]; then
  if [[ "$OS" == Darwin ]] && /usr/bin/arch "-$ARCH" /usr/bin/true 2> /dev/null; then
    runner=(/usr/bin/arch "-$ARCH")
  else
    can_run=0
    echo "warning: can't run $ARCH programs here, so the build isn't checked." >&2
  fi
fi
run_out() {
  ${runner[@]+"${runner[@]}"} "$@"
}
if [[ "$can_run" == 1 ]]; then
  # Read whole before searching: grep -q stopping early would fail the
  # pipeline when FFmpeg can't finish writing.
  licence="$(run_out "$OUT/ffmpeg" -hide_banner -L)"
  grep -q "Lesser General Public License" <<< "$licence" || {
    echo "This FFmpeg isn't LGPL." >&2
    exit 1
  }
  encoders="$(run_out "$OUT/ffmpeg" -hide_banner -encoders)"
  wanted=(libopus libvorbis libvpx-vp9 aac flac)
  [[ "$SKIP" != *" lame "* ]] && wanted+=(libmp3lame)
  [[ "$OS" == Darwin ]] && wanted+=(h264_videotoolbox aac_at)
  for encoder in "${wanted[@]}"; do
    grep -qw "$encoder" <<< "$encoders" || { echo "No $encoder encoder." >&2; exit 1; }
  done
  filters="$(run_out "$OUT/ffmpeg" -hide_banner -filters)"
  grep -qw subtitles <<< "$filters" || { echo "No subtitles filter." >&2; exit 1; }
  decoders="$(run_out "$OUT/ffmpeg" -hide_banner -decoders)"
  grep -qw libdav1d <<< "$decoders" || { echo "No AV1 decoder." >&2; exit 1; }
  run_out "$OUT/ffprobe" -hide_banner -version > /dev/null
fi
if [[ "$OS" == Darwin ]]; then
  for tool in ffmpeg ffprobe; do
    # Only the system's libraries and frameworks.
    if otool -L "$OUT/$tool" | tail -n +2 | grep -vE '^[[:space:]]+(/usr/lib/|/System/Library/)'; then
      echo "$tool links a library outside the system." >&2
      exit 1
    fi
    lipo "$OUT/$tool" -verify_arch "$ARCH"
  done
fi
echo "Built $OUT/ffmpeg and $OUT/ffprobe"
