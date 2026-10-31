#!/usr/bin/env bash
# Interactive installer/uninstaller for arctis_chatmix
# Usage:
#   ./install.sh
#   ./install.sh --binary ./arctis_chatmix --mode user --udev yes --enable-service yes
#   ./install.sh --uninstall
#   ./install.sh --uninstall --mode user --udev yes
set -euo pipefail

# Defaults
BINARY="./arctis_chatmix"
MODE="user"          # user or system
INSTALL_UDEV="yes"   # yes/no
ENABLE_SERVICE="yes" # yes/no
ENABLE_LINGER="no"   # yes/no (only relevant for user mode)
UNINSTALL="no"       # yes/no
SKIP_CONFIRM="no"    # yes/no — skip all interactive prompts (-y/--yes)

print_help() {
  cat <<'USAGE'
Usage: install.sh [OPTIONS]
Interactive installer/uninstaller for arctis_chatmix.
Options (non-interactive):
  --binary PATH           Path to the arctis_chatmix binary (default: ./arctis_chatmix)
  --mode user|system      Install as a per-user service (default) or system-wide service
  --udev yes|no           Install/remove udev rule (default: yes)
  --enable-service yes|no Enable and start the service immediately (default: yes)
  --enable-linger yes|no  Enable systemd linger for the user (only relevant for --mode user; default: no)
  --uninstall             Remove arctis_chatmix (service, binary, and optionally udev rule)
  -y, --yes               Skip all interactive prompts and use provided/default values
  -h, --help              Show this help and exit
USAGE
}

# Parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
  --binary)
    shift
    BINARY="$1"
    shift
    ;;
  --mode)
    shift
    MODE="$1"
    shift
    ;;
  --udev)
    shift
    INSTALL_UDEV="$1"
    shift
    ;;
  --enable-service)
    shift
    ENABLE_SERVICE="$1"
    shift
    ;;
  --enable-linger)
    shift
    ENABLE_LINGER="$1"
    shift
    ;;
  --uninstall)
    UNINSTALL="yes"
    shift
    ;;
  -y | --yes)
    SKIP_CONFIRM="yes"
    shift
    ;;
  -h | --help)
    print_help
    exit 0
    ;;
  *)
    echo "Unknown argument: $1"
    print_help
    exit 2
    ;;
  esac
done

ask_yes_no() {
  local prompt="$1" default="$2" reply
  while true; do
    read -r -p "$prompt [$default] " reply || exit 1
    reply="${reply:-$default}"
    case "${reply,,}" in
    y | yes)
      echo "yes"
      return 0
      ;;
    n | no)
      echo "no"
      return 0
      ;;
    *) echo "Please answer yes or no (y/n)." ;;
    esac
  done
}

# Run a command with sudo if not already root.
run_privileged() {
  if [[ $EUID -ne 0 ]]; then
    sudo "$@"
  else
    "$@"
  fi
}

IS_TTY=1
[[ ! -t 0 ]] && IS_TTY=0

# ── User & Path resolution (handles running via sudo) ─────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REAL_USER="${SUDO_USER:-$(id -un)}"
REAL_HOME="$(getent passwd "$REAL_USER" 2>/dev/null | cut -d: -f6 || true)"
[[ -z "$REAL_HOME" ]] && REAL_HOME="$HOME"

SYSTEM_BIN_DIR="/usr/local/bin"
USER_BIN_DIR="$REAL_HOME/.local/bin"
if [[ -n "${XDG_CONFIG_HOME:-}" && "$XDG_CONFIG_HOME" != "/root/.config" ]]; then
  CONFIG_DIR="$XDG_CONFIG_HOME/arctis-nova-7"
else
  CONFIG_DIR="$REAL_HOME/.config/arctis-nova-7"
fi
SERVICE_NAME="arctis_chatmix.service"
USER_UNIT_DIR="$REAL_HOME/.config/systemd/user"
SYSTEM_UNIT_DIR="/etc/systemd/system"
UDEV_RULE_PATH="/etc/udev/rules.d/99-arctis.rules"
WEB_DASHBOARD_URL="http://127.0.0.1:3927"

# All 20 SteelSeries Arctis Nova 7 RX & TX Product IDs (Engine & Companion Database)
PIDS=(
  "2200" "2202" "2204" "2206" "2208" "220a"
  "2238" "223a" "2256" "2258" "2278" "227a"
  "227c" "227e" "2288" "228a" "229c" "229e"
  "22a1" "22a9"
)

# ══════════════════════════════════════════════════════════════════════════════
# UNINSTALL
# ══════════════════════════════════════════════════════════════════════════════

uninstall_user() {
  echo "Stopping and disabling user service..."
  systemctl --user stop  "$SERVICE_NAME" 2>/dev/null || true
  systemctl --user disable "$SERVICE_NAME" 2>/dev/null || true

  local unit_file="$USER_UNIT_DIR/$SERVICE_NAME"
  if [[ -f "$unit_file" ]]; then
    rm -f "$unit_file"
    echo "Removed unit: $unit_file"
  else
    echo "Unit file not found (already removed?): $unit_file"
  fi

  systemctl --user daemon-reload

  local bin_file="$USER_BIN_DIR/arctis_chatmix"
  if [[ -f "$bin_file" ]]; then
    rm -f "$bin_file"
    echo "Removed binary: $bin_file"
  else
    echo "Binary not found (already removed?): $bin_file"
  fi
}

uninstall_system() {
  echo "Stopping and disabling system service (requires sudo)..."
  run_privileged systemctl stop    "$SERVICE_NAME" 2>/dev/null || true
  run_privileged systemctl disable "$SERVICE_NAME" 2>/dev/null || true

  local unit_file="$SYSTEM_UNIT_DIR/$SERVICE_NAME"
  if [[ -f "$unit_file" ]]; then
    run_privileged rm -f "$unit_file"
    echo "Removed unit: $unit_file"
  else
    echo "Unit file not found (already removed?): $unit_file"
  fi

  run_privileged systemctl daemon-reload

  local bin_file="$SYSTEM_BIN_DIR/arctis_chatmix"
  if [[ -f "$bin_file" ]]; then
    run_privileged rm -f "$bin_file"
    echo "Removed binary: $bin_file"
  else
    echo "Binary not found (already removed?): $bin_file"
  fi
}

uninstall_udev() {
  if [[ "$INSTALL_UDEV" != "yes" ]]; then
    echo "Skipping udev rule removal (--udev no)"
    return
  fi
  if [[ -f "$UDEV_RULE_PATH" ]]; then
    echo "Removing udev rule (requires sudo)..."
    run_privileged rm -f "$UDEV_RULE_PATH"
    run_privileged udevadm control --reload
    echo "Removed udev rule: $UDEV_RULE_PATH"
  else
    echo "udev rule not found (already removed?): $UDEV_RULE_PATH"
  fi
}

uninstall_rtkit() {
  if [[ "$SKIP_CONFIRM" != "yes" ]]; then
    if [[ "$(ask_yes_no "Remove rtkit (RealtimeKit)? WARNING: Other audio apps may depend on it!" no)" != "yes" ]]; then
      echo "Skipping rtkit removal."
      return
    fi
  fi

  echo "Removing rtkit (requires sudo)..."
  if command -v pacman >/dev/null 2>&1; then
    run_privileged pacman -Rns --noconfirm rtkit || true
  elif command -v apt-get >/dev/null 2>&1; then
    run_privileged apt-get remove -y rtkit || true
  elif command -v dnf >/dev/null 2>&1; then
    run_privileged dnf remove -y rtkit || true
  fi
  echo "rtkit removed."
}

run_uninstall() {
  echo "== arctis_chatmix uninstaller =="

  # Interactive mode: ask questions only when not suppressed by --yes
  if [[ $IS_TTY -eq 1 && "$SKIP_CONFIRM" != "yes" ]]; then
    read -r -p "Uninstall mode - user or system [$MODE]: " in || exit 1
    MODE="${in:-$MODE}"
    INSTALL_UDEV=$(ask_yes_no "Also remove udev rule?" "$INSTALL_UDEV")
    [[ "$(ask_yes_no "Proceed with uninstall?" yes)" != "yes" ]] && exit 0
  fi

  if [[ "$MODE" == "user" ]]; then
    uninstall_user
  else
    uninstall_system
  fi
  uninstall_udev
  uninstall_rtkit

  echo ""
  echo "✓ Uninstall complete."
  echo "  Note: User configuration and EQ profiles in $CONFIG_DIR were preserved."
  echo "  Virtual sinks (Arctis_Game / Arctis_Chat) will disappear"
  echo "  after your PipeWire session restarts or on next login."
}

# ══════════════════════════════════════════════════════════════════════════════
# INSTALL
# ══════════════════════════════════════════════════════════════════════════════

ensure_audio_group() {
  getent group audio >/dev/null && return
  echo "Creating audio group (requires sudo)..."
  run_privileged groupadd -r audio
}

add_user_to_audio_group() {
  local u="$REAL_USER"
  [[ "$u" == "root" ]] && return
  id -nG "$u" | grep -qw audio && return
  echo "Adding user '$u' to audio group (requires sudo)..."
  run_privileged usermod -aG audio "$u"
  echo "IMPORTANT: You must log out and log back in (or reboot)."
}

init_user_config() {
  local bin_path="$1"
  if [[ $EUID -eq 0 && "$REAL_USER" != "root" ]]; then
    sudo -u "$REAL_USER" HOME="$REAL_HOME" mkdir -p "$CONFIG_DIR/profiles"
    if [[ -x "$bin_path" ]]; then
      sudo -u "$REAL_USER" HOME="$REAL_HOME" "$bin_path" --init-config 2>/dev/null || true
    fi
  else
    mkdir -p "$CONFIG_DIR/profiles"
    if [[ -x "$bin_path" ]]; then
      "$bin_path" --init-config 2>/dev/null || true
    fi
  fi
}

install_user() {
  mkdir -p "$USER_BIN_DIR" "$USER_UNIT_DIR"
  install -m 755 "$BINARY" "$USER_BIN_DIR/arctis_chatmix"
  init_user_config "$USER_BIN_DIR/arctis_chatmix"
  cat >"${USER_UNIT_DIR}/${SERVICE_NAME}" <<'UNIT'
[Unit]
Description=Arctis Nova 7 ChatMix, 10-Band DSP EQ & Web Companion Service
Documentation=https://github.com/m24ih/Arctis-Nova-7-Chatmix
Wants=pipewire.service
After=pipewire.service
[Service]
ExecStart=%h/.local/bin/arctis_chatmix
Environment=ARCTIS_SIDETONE_DISABLE=1
Environment=RUST_LOG=info
Restart=on-failure
RestartSec=5
[Install]
WantedBy=default.target
UNIT
  if [[ $EUID -eq 0 && "$REAL_USER" != "root" ]]; then
    chown -R "$REAL_USER":"$(id -gn "$REAL_USER")" "$USER_BIN_DIR/arctis_chatmix" "${USER_UNIT_DIR}/${SERVICE_NAME}" 2>/dev/null || true
  fi
  systemctl --user daemon-reload 2>/dev/null || true
  [[ "$ENABLE_SERVICE" == "yes" ]] && systemctl --user enable --now "$SERVICE_NAME" || true
  [[ "$ENABLE_LINGER" == "yes" ]] &&
    run_privileged loginctl enable-linger "$REAL_USER" || true
}

install_system() {
  echo "Installing system-wide (requires sudo)..."
  run_privileged install -m 755 "${BINARY}" "${SYSTEM_BIN_DIR}/arctis_chatmix"
  echo "Binary installed to ${SYSTEM_BIN_DIR}/arctis_chatmix"
  init_user_config "${SYSTEM_BIN_DIR}/arctis_chatmix"

  local unit_content
  read -r -d '' unit_content <<'UNIT' || true
[Unit]
Description=Arctis Nova 7 ChatMix, 10-Band DSP EQ & Web Companion Service
Documentation=https://github.com/m24ih/Arctis-Nova-7-Chatmix
Wants=pipewire.service
After=pipewire.service
[Service]
Type=simple
ExecStart=/usr/local/bin/arctis_chatmix
Environment=ARCTIS_SIDETONE_DISABLE=1
Environment=RUST_LOG=info
Restart=on-failure
RestartSec=5
[Install]
WantedBy=multi-user.target
UNIT

  echo "$unit_content" | run_privileged tee "${SYSTEM_UNIT_DIR}/${SERVICE_NAME}" >/dev/null
  run_privileged systemctl daemon-reload
  [[ "${ENABLE_SERVICE}" == "yes" ]] && run_privileged systemctl enable --now "$SERVICE_NAME"
}

install_udev() {
  if [[ "${INSTALL_UDEV}" != "yes" ]]; then
    echo "Skipping udev rule install (--udev no)"
    return
  fi
  echo "Installing udev rule (requires sudo)..."
  if [[ -f "${SCRIPT_DIR}/config/99-arctis.rules" ]]; then
    run_privileged install -Dm644 "${SCRIPT_DIR}/config/99-arctis.rules" "${UDEV_RULE_PATH}"
  elif [[ -f "./config/99-arctis.rules" ]]; then
    run_privileged install -Dm644 "./config/99-arctis.rules" "${UDEV_RULE_PATH}"
  else
    UDEV_CONTENT=""
    for pid in "${PIDS[@]}"; do
      UDEV_CONTENT+='ATTRS{idVendor}=="1038", ATTRS{idProduct}=="'"$pid"'", MODE="0660", GROUP="audio", TAG+="uaccess"
KERNEL=="hidraw*", ATTRS{idVendor}=="1038", ATTRS{idProduct}=="'"$pid"'", MODE="0660", GROUP="audio", TAG+="uaccess"
ACTION=="add", SUBSYSTEM=="usb", ATTR{idVendor}=="1038", ATTR{idProduct}=="'"$pid"'", TEST=="power/control", ATTR{power/control}="on"
'
    done
    echo "$UDEV_CONTENT" | run_privileged tee "${UDEV_RULE_PATH}" >/dev/null
  fi
  run_privileged udevadm control --reload
  for pid in "${PIDS[@]}"; do
    run_privileged udevadm trigger --subsystem-match=usb --attr-match=idVendor=1038 --attr-match=idProduct="$pid" || true
    run_privileged udevadm trigger --subsystem-match=hidraw --attr-match=idVendor=1038 --attr-match=idProduct="$pid" 2>/dev/null || true
  done
  echo "udev rule installed to ${UDEV_RULE_PATH}"
  echo "Make sure your user is in the 'audio' group (sudo usermod -aG audio <user>) and re-login."
}

install_rtkit() {
  echo "Checking for rtkit (RealtimeKit)..."
  if systemctl status rtkit-daemon.service >/dev/null 2>&1; then
    echo "rtkit is already installed and running."
    return
  fi

  echo "rtkit is not running. Attempting to install and enable it (requires sudo)..."
  if command -v pacman >/dev/null 2>&1; then
    run_privileged pacman -S --noconfirm rtkit
  elif command -v apt-get >/dev/null 2>&1; then
    run_privileged apt-get install -y rtkit
  elif command -v dnf >/dev/null 2>&1; then
    run_privileged dnf install -y rtkit
  else
    echo "[warn] Could not determine package manager. Please install 'rtkit' manually."
    return
  fi

  run_privileged systemctl enable --now rtkit-daemon.service || true
  echo "rtkit installed and enabled."
}

run_install() {
  echo "== arctis_chatmix installer =="

  # Auto-detect cargo release binary if default ./arctis_chatmix is missing
  if [[ "$BINARY" == "./arctis_chatmix" && ! -f "$BINARY" && -f "${SCRIPT_DIR}/target/release/arctis_chatmix" ]]; then
    BINARY="${SCRIPT_DIR}/target/release/arctis_chatmix"
  fi

  if [[ $IS_TTY -eq 1 && "$SKIP_CONFIRM" != "yes" ]]; then
    read -r -p "Path to binary [$BINARY]: " in || exit 1
    BINARY="${in:-$BINARY}"
    [[ ! -f "$BINARY" ]] && echo "Binary not found: $BINARY" && exit 2
    read -r -p "Install mode - user or system [$MODE]: " in || exit 1
    MODE="${in:-$MODE}"
    INSTALL_UDEV=$(ask_yes_no "Install udev rule?" "$INSTALL_UDEV")
    ENABLE_SERVICE=$(ask_yes_no "Enable & start service now?" "$ENABLE_SERVICE")
    [[ "$MODE" == "user" ]] &&
      ENABLE_LINGER=$(ask_yes_no "Enable linger (loginctl enable-linger)?" "$ENABLE_LINGER")
    [[ "$(ask_yes_no "Proceed?" yes)" != "yes" ]] && exit 0
  fi

  if [[ ! -f "$BINARY" ]]; then
    echo "Binary not found: $BINARY"
    exit 2
  fi

  if [[ "$MODE" == "user" ]]; then
    install_user
  else
    install_system
  fi
  ensure_audio_group   || echo "[warn] Could not ensure audio group (run manually: sudo groupadd -r audio)"
  add_user_to_audio_group || echo "[warn] Could not add user to audio group (run manually: sudo usermod -aG audio $REAL_USER)"
  install_udev
  install_rtkit
  echo ""
  echo "✓ Installation complete."
  echo "  • Config & EQ Profiles: $CONFIG_DIR"
  echo "  • Web Companion UI:     $WEB_DASHBOARD_URL"
  echo "  • If audio group was modified: LOG OUT and LOG BACK IN."
}

# ── Entry point ───────────────────────────────────────────────────────────────
if [[ "$UNINSTALL" == "yes" ]]; then
  run_uninstall
else
  run_install
fi

