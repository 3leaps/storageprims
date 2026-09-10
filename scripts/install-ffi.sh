#!/usr/bin/env bash
# Install or uninstall the exact local Unix FFI artifact set.

set -euo pipefail

mode="${1:?usage: install-ffi.sh MODE source-dir header libdir includedir}"
source_dir="${2:?missing source directory}"
header="${3:?missing header}"
libdir="${4:?missing library directory}"
includedir="${5:?missing include directory}"

case "$(uname -s)" in
Darwin) shared="libstorageprims_ffi.dylib" ;;
Linux) shared="libstorageprims_ffi.so" ;;
*)
	echo "error: local FFI install supports Unix hosts only" >&2
	exit 1
	;;
esac

for directory in "$libdir" "$includedir"; do
	if [[ -z "$directory" || "$directory" != /* || "$directory" == "/" ||
		"$directory" == *"/.." || "$directory" == *"/../"* ||
		"$directory" == *"/." || "$directory" == *"/./"* ]]; then
		echo "error: refusing unsafe install directory" >&2
		exit 1
	fi
	if [[ -L "$directory" ]]; then
		echo "error: refusing symlink install directory" >&2
		exit 1
	fi
done

static_dest="$libdir/libstorageprims_ffi.a"
shared_dest="$libdir/$shared"
header_dest="$includedir/storageprims.h"
if [[ "$source_dir" == "$libdir" || "$header" == "$header_dest" ]]; then
	echo "error: install sources and destinations must be distinct" >&2
	exit 1
fi

case "$mode" in
install)
	for source in \
		"$source_dir/libstorageprims_ffi.a" \
		"$source_dir/$shared" \
		"$header"; do
		[[ -f "$source" && ! -L "$source" ]] || {
			echo "error: required FFI install input is missing or unsafe" >&2
			exit 1
		}
	done
	mkdir -p "$libdir" "$includedir"
	rm -f -- "$static_dest" "$shared_dest" "$header_dest"
	cp "$source_dir/libstorageprims_ffi.a" "$static_dest"
	cp "$source_dir/$shared" "$shared_dest"
	cp "$header" "$header_dest"
	chmod 0644 "$static_dest" "$shared_dest" "$header_dest"
	echo "[ok] storageprims FFI installed"
	;;
uninstall)
	rm -f -- "$static_dest" "$shared_dest" "$header_dest"
	echo "[ok] storageprims FFI uninstalled"
	;;
*)
	echo "error: mode must be install or uninstall" >&2
	exit 1
	;;
esac
