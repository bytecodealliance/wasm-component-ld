#!/bin/bash

set -ex

platform=$1
target=$2

rm -rf tmp
mkdir tmp
mkdir -p dist

tag=v$(./ci/print-current-version.sh)
bin_pkgname=wasm-component-ld-$tag-$platform

mkdir tmp/$bin_pkgname
cp LICENSE-* README.md tmp/$bin_pkgname

fmt=tar
if [ "$platform" = "x86_64-windows" ]; then
  cp target/release/wasm-component-ld.exe tmp/$bin_pkgname
  fmt=zip
elif [ "$target" = "" ]; then
  cp target/release/wasm-component-ld tmp/$bin_pkgname
else
  cp target/$target/release/wasm-component-ld tmp/$bin_pkgname
fi


mktarball() {
  dir=$1
  if [ "$fmt" = "tar" ]; then
    tar czvf dist/$dir.tar.gz -C tmp $dir
  else
    # Note that this runs on Windows, and it looks like GitHub Actions doesn't
    # have a `zip` tool there, so we use something else
    (cd tmp && 7z a ../dist/$dir.zip $dir/)
  fi
}

mktarball $bin_pkgname
