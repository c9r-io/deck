#!/bin/sh
# Reproducible static tmux for the deck sidecar (macOS arm64).
# Statically links libevent, ncurses and utf8proc; the result depends only
# on /usr/lib system libraries. Run on the target architecture.
set -e
WORK=${1:-$(mktemp -d)}
PREFIX="$WORK/prefix"
JOBS=$(sysctl -n hw.ncpu)
cd "$WORK"

LIBEVENT=2.1.12-stable
NCURSES=6.5
UTF8PROC=2.9.0

curl -sL -o libevent.tgz "https://github.com/libevent/libevent/releases/download/release-$LIBEVENT/libevent-$LIBEVENT.tar.gz"
curl -sL -o ncurses.tgz "https://ftp.gnu.org/gnu/ncurses/ncurses-$NCURSES.tar.gz"
curl -sL -o utf8proc.tgz "https://github.com/JuliaStrings/utf8proc/archive/refs/tags/v$UTF8PROC.tar.gz"
TMUX_TAG=3.7c
curl -sL -o tmux.tgz "https://github.com/tmux/tmux/releases/download/$TMUX_TAG/tmux-$TMUX_TAG.tar.gz"
echo "building tmux $TMUX_TAG"

# supply-chain pinning: refuse to build from tarballs we haven't reviewed
shasum -a 256 -c <<'SUMS'
92e6de1be9ec176428fd2367677e61ceffc2ee1cb119035037a27d346b0403bb  libevent.tgz
136d91bc269a9a5785e5f9e980bc76ab57428f604ce3e5a5a90cebc767971cc6  ncurses.tgz
7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf  tmux.tgz
18c1626e9fc5a2e192311e36b3010bfc698078f692888940f1fa150547abb0c1  utf8proc.tgz
SUMS

tar xzf libevent.tgz && cd "libevent-$LIBEVENT"
./configure --prefix="$PREFIX" --disable-shared --enable-static --disable-openssl --disable-samples >/dev/null
make -j"$JOBS" >/dev/null && make install >/dev/null
cd ..

tar xzf ncurses.tgz && cd "ncurses-$NCURSES"
./configure --prefix="$PREFIX" --without-shared --without-debug --without-ada \
  --without-manpages --without-progs --without-tests --without-cxx-binding --disable-db-install \
  --with-default-terminfo-dir=/usr/share/terminfo \
  --with-terminfo-dirs="/usr/share/terminfo:/opt/homebrew/share/terminfo:/usr/local/share/terminfo" >/dev/null
make -j"$JOBS" >/dev/null && make install >/dev/null
cd ..

tar xzf utf8proc.tgz && cd "utf8proc-$UTF8PROC"
make -j"$JOBS" prefix="$PREFIX" >/dev/null && make prefix="$PREFIX" install >/dev/null
rm -f "$PREFIX"/lib/libutf8proc*.dylib
cd ..

tar xzf tmux.tgz && cd tmux-*/
PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig" \
CPPFLAGS="-I$PREFIX/include -I$PREFIX/include/ncurses" \
LDFLAGS="-L$PREFIX/lib" \
./configure --prefix="$PREFIX" --enable-utf8proc --disable-jemalloc >/dev/null
make -j"$JOBS" >/dev/null
strip tmux
cp tmux "$WORK/tmux-static"
echo "=== otool -L:"
otool -L "$WORK/tmux-static"
echo "built: $WORK/tmux-static ($($WORK/tmux-static -V))"
