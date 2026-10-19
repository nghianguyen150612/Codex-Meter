#!/usr/bin/env bash
# Codex Meter v0.1.0 user-local installer (Linux x86_64; macOS x86_64/arm64).
set -euo pipefail

VERSION="0.1.0"
RELEASE="https://github.com/nghianguyen150612/Codex-Meter/releases/download/v$VERSION"
WHEEL="codex_meter_analytics-0.1.0-py3-none-any.whl"
WHEEL_SHA="98777ada1c620ecc390b8d1f67d0c6b5312b13e30b6211eff95b33879e62c39f"
PREFIX="$HOME/.local/share/codex-meter"
COMMAND_DIR="$HOME/.local/bin"

die() { printf 'codex-meter install: %s\n' "$*" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"
command -v install >/dev/null 2>&1 || die "install is required"

case "$(uname -s):$(uname -m)" in
  Linux:x86_64|Linux:amd64)
    TARGET="linux-x86_64"
    ARCHIVE_SHA="a0f14b1fb10c38f971113363ada8bf0fc9fe4f5a7c1c7998b358e541c443b758"
    ;;
  Darwin:x86_64)
    TARGET="macos-x86_64"
    ARCHIVE_SHA="0bf11a808375a8578b9dbde755cf9f100675941d6a2b448f103872bbcd6cb06c"
    ;;
  Darwin:arm64)
    TARGET="macos-arm64"
    ARCHIVE_SHA="1789d1385b6d8be18a95f77526ee01c5e48d6feb300c765266daee8e3429ba48"
    ;;
  *) die "unsupported platform: $(uname -s) / $(uname -m)" ;;
esac

ARCHIVE="codex-meter-$VERSION-$TARGET.tar.gz"
TMP="$(mktemp -d)" || die "cannot create temporary directory"
trap 'rm -rf "$TMP"' EXIT

download() {
  curl --fail --location --silent --show-error --retry 2 \
    --proto '=https' --tlsv1.2 \
    --output "$TMP/$1" "$RELEASE/$1"
}
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "sha256sum or shasum is required"
  fi
}
verify() {
  actual="$(sha256 "$TMP/$1")"
  [ "$actual" = "$2" ] || die "SHA-256 verification failed for $1; nothing installed"
}

printf 'Downloading verified Codex Meter %s assets for %s...\n' "$VERSION" "$TARGET"
download "$ARCHIVE"
download "$WHEEL"
verify "$ARCHIVE" "$ARCHIVE_SHA"
verify "$WHEEL" "$WHEEL_SHA"

PYTHON="$(printenv CODEX_METER_PYTHON 2>/dev/null || true)"
if [ -n "$PYTHON" ]; then
  "$PYTHON" -c 'import sys; assert (3,11) <= sys.version_info[:2] <= (3,13)' \
    || die "CODEX_METER_PYTHON must point to Python 3.11–3.13"
else
  PYTHON=""
  for candidate in python3.13 python3.12 python3.11; do
    if command -v "$candidate" >/dev/null 2>&1 && \
       "$candidate" -c 'import sys; assert (3,11) <= sys.version_info[:2] <= (3,13)' >/dev/null 2>&1; then
      PYTHON="$(command -v "$candidate")"
      break
    fi
  done
  if [ -z "$PYTHON" ] && command -v uv >/dev/null 2>&1; then
    printf 'Installing user-local Python 3.13 through uv...\n'
    uv python install 3.13 || die "could not install Python 3.13 with uv"
    PYTHON="$(uv python find 3.13)" || die "uv could not locate Python 3.13"
  fi
  [ -n "$PYTHON" ] || die "install Python 3.11–3.13 or uv and retry"
fi

tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
BINARY="$TMP/codex-meter-$VERSION-$TARGET/codex-meter"
[ -f "$BINARY" ] && [ ! -L "$BINARY" ] || die "expected executable missing from archive"
mkdir -p "$PREFIX/bin" "$COMMAND_DIR"

VENV="$PREFIX/venv"
if [ ! -f "$VENV/pyvenv.cfg" ]; then
  "$PYTHON" -m venv "$VENV" || die "could not create Python venv; install venv support"
fi
VENV_PYTHON="$VENV/bin/python"
[ -x "$VENV_PYTHON" ] || die "venv Python executable missing"
"$VENV_PYTHON" -c 'import sys; assert (3,11) <= sys.version_info[:2] <= (3,13)' \
  || die "existing Codex Meter venv uses unsupported Python; remove it before retrying"

if command -v uv >/dev/null 2>&1; then
  uv pip install --python "$VENV_PYTHON" --no-deps --no-index \
    --reinstall "$TMP/$WHEEL" || die "wheel installation failed"
else
  "$VENV_PYTHON" -m pip install --no-index --no-deps \
    --force-reinstall "$TMP/$WHEEL" || die "wheel installation failed"
fi

"$VENV_PYTHON" -c 'from importlib.metadata import version; assert version("codex-meter-analytics") == "0.1.0"' \
  || die "installed wheel version verification failed"
install -m 0755 "$BINARY" "$PREFIX/bin/codex-meter"

# Supply the pinned installed interpreter only for the estimate subcommand.
cat > "$COMMAND_DIR/codex-meter" <<'WRAPPER'
#!/bin/sh
set -eu
binary="$HOME/.local/share/codex-meter/bin/codex-meter"
python="$HOME/.local/share/codex-meter/venv/bin/python"
if [ "$#" -gt 0 ] && [ "$1" = "estimate" ]; then
  exec "$binary" "$@" --worker-mode installed --python "$python"
fi
exec "$binary" "$@"
WRAPPER
chmod 0755 "$COMMAND_DIR/codex-meter"

"$COMMAND_DIR/codex-meter" --version
printf '\nInstalled Codex Meter %s.\nCommand: %s/codex-meter\n' "$VERSION" "$COMMAND_DIR"
printf 'Add %s to your PATH if needed.\n' "$COMMAND_DIR"
printf 'Usage: codex-meter estimate --database PATH --request FILE --generated-at UTC [--five-hour-used-percent N]\n'
printf 'Note: estimates require an existing Codex Meter SQLite database; automatic tracking is not included.\n'