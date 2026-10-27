#!/usr/bin/env bash
set -euo pipefail

source_dir=${1:?Pass the extracted FFmpeg 9.0.2 source directory}
build_root=${2:-$source_dir}
jobs=${3:-8}
mingw_bin=${FFMPEG_GCC_BIN:-/c/PROGRA~1/mingw64/bin}

to_unix_path() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

source_dir=$(to_unix_path "$source_dir")
build_root=$(to_unix_path "$build_root")
mingw_bin=$(to_unix_path "$mingw_bin")
export PATH="$mingw_bin:$PATH"
mkdir -p "$build_root"
cd "$source_dir"

./configure \
    --prefix="$build_root/install" \
    --target-os=mingw32 --arch=x86_64 \
    --cc="$mingw_bin/gcc.exe" \
    --ar="$mingw_bin/ar.exe" \
    --ranlib="$mingw_bin/ranlib.exe" \
    --windres="$mingw_bin/windres.exe" \
    --disable-everything --disable-autodetect --disable-gpl --disable-nonfree \
    --disable-shared --enable-static --disable-doc --disable-debug \
    --disable-ffplay --disable-ffprobe --disable-avdevice --disable-x86asm \
    --enable-ffmpeg --enable-schannel --enable-w32threads --disable-pthreads \
    --enable-demuxer=hls,mpegts,mov,aac,flv \
    --enable-muxer=flv,matroska \
    --enable-decoder=aac,ac3,eac3,h264,pcm_s16le \
    --enable-encoder=pcm_s16le \
    --enable-parser=aac,ac3,h264 \
    --enable-bsf=extract_extradata,aac_adtstoasc,h264_mp4toannexb,null \
    --enable-filter=aresample,anull,abuffer,abuffersink,aformat \
    --enable-protocol=file,pipe,crypto,tcp,tls,http,https,udp \
    --extra-ldflags=-static

mingw32-make -j"$jobs" V=1
