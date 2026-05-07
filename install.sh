#!/usr/bin/env bash
# Interactive installer for arctis_chatmix
# Usage:
#   ./install.sh
#   ./install.sh --binary ./arctis_chatmix --mode user --udev yes --enable-service yes
set -euo pipefail

# Defaults
BINARY="./arctis_chatmix"
MODE="user"          # user or system
INSTALL_UDEV="yes"   # yes/no
ENABLE_SERVICE="yes" # yes/no
ENABLE_LINGER="no"   # yes/no (only relevant for user mode)

print_help() {
  cat <<'USAGE'
Usage: install.sh [OPTIONS]

Interactive installer for arctis_chatmix.

Options (non-interactive):
  --binary PATH           Path to the arctis_chatmix binary (default: ./arctis_chatmix)
  --mode user|system      Install as a per-user service (default) or system-wide service
  --udev yes|no           Install udev rule (default: yes)
  --enable-service yes|no Enable and start the service immediately (default: yes)
  --enable-linger yes|no  Enable systemd linger for the user (only relevant for --mode user; default: no)
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
    shifthttps://github.com/Lairizzle/Arctis-Nova-7-Wireless-Chatmix/pull/1/conflict?name=install.sh&ancestor_oid=704fb711a5b885c0de9ecb539a3573ec0bcb25cd&base_oid=4d1b10c4ea5672ead3d76bd2ea2b9ca493cc26ce&head_oid=eed497195f827b94188c4fbf67e3fdf564bc9aa6
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

IS_TTY=1
[[ ! -t 0 ]] && IS_TTY=0

if [[ $IS_TTY -eq 1 ]]; then
  echo "Arctis ChatMix installer (interactive)"

  read -r -p "Path to binary [$BINARY]: " in || exit 1
  BINARY="${in:-$BINARY}"
  [[ ! -f "$BINARY" ]] && echo "Binary not found." && exit 2

  read -r -p "Install mode - user or system [$MODE]: " in || exit 1
  MODE="${in:-$MODE}"

  INSTALL_UDEV=$(ask_yes_no "Install udev rule?" "$INSTALL_UDEV")
  ENABLE_SERVICE=$(ask_yes_no "Enable & start service now?" "$ENABLE_SERVICE")

  [[ "$MODE" == "user" ]] &&
    ENABLE_LINGER=$(ask_yes_no "Enable linger (loginctl enable-linger)?" "$ENABLE_LINGER")
fi

SYSTEM_BIN_DIR="/usr/local/bin"
USER_BIN_DIR="$HOME/.local/bin"
SERVICE_NAME="arctis_chatmix.service"
USER_UNIT_DIR="$HOME/.config/systemd/user"
SYSTEM_UNIT_DIR="/etc/systemd/system"
UDEV_RULE_PATH="/etc/udev/rules.d/99-arctis.rules"

ensure_audio_group() {
  getent group audio >/dev/null && return
  echo "Creating audio group (requires sudo)..."
  [[ $EUID -ne 0 ]] && sudo groupadd -r audio || groupadd -r audio
}

add_user_to_audio_group() {
  local u
  u="$(id -un)"
  id -nG "$u" | grep -qw audio && return
  echo "Adding user '$u' to audio group (requires sudo)..."
  [[ $EUID -ne 0 ]] && sudo usermod -aG audio "$u" || usermod -aG audio "$u"
  echo "IMPORTANT: You must log out and log back in (or reboot)."
}

install_user() {
  mkdir -p "$USER_BIN_DIR" "$USER_UNIT_DIR"
  install -m 755 "$BINARY" "$USER_BIN_DIR/arctis_chatmix"

  mkdir -p "${USER_UNIT_DIR}"
  cat >"${USER_UNIT_DIR}/${SERVICE_NAME}" <<'UNIT'
[Unit]
Description=Arctis Nova 7 ChatMix (virtual-sink mixer)
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

  systemctl --user daemon-reload
  [[ "$ENABLE_SERVICE" == "yes" ]] && systemctl --user enable --now "$SERVICE_NAME" || true

  [[ "$ENABLE_LINGER" == "yes" ]] &&
    ([[ $EUID -ne 0 ]] && sudo loginctl enable-linger "$(id -un)" || loginctl enable-linger "$(id -un)")
}

install_system() {
  echo "Installing system-wide (requires sudo)..."
  if [[ $EUID -ne 0 ]]; then
    sudo install -m 755 "${BINARY}" "${SYSTEM_BIN_DIR}/arctis_chatmix"
  else
    install -m 755 "${BINARY}" "${SYSTEM_BIN_DIR}/arctis_chatmix"
  fi
  echo "Binary installed to ${SYSTEM_BIN_DIR}/arctis_chatmix"

  # write systemd unit
  if [[ $EUID -ne 0 ]]; then
    sudo tee "${SYSTEM_UNIT_DIR}/${SERVICE_NAME}" >/dev/null <<'UNIT'
[Unit]
Description=Arctis Nova 7 ChatMix (virtual-sink mixer)
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
    sudo systemctl daemon-reload
    if [[ "${ENABLE_SERVICE}" == "yes" ]]; then
      sudo systemctl enable --now arctis_chatmix.service
    fi
  else
    tee "${SYSTEM_UNIT_DIR}/${SERVICE_NAME}" >/dev/null <<'UNIT'
[Unit]
Description=Arctis Nova 7 ChatMix (virtual-sink mixer)
Wants=pipewire.service
After=pipewire.service

[Service]
ExecStart=/usr/local/bin/arctis_chatmix
Environment=ARCTIS_SIDETONE_DISABLE=1
Environment=RUST_LOG=info
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT

  [[ $EUID -ne 0 ]] && sudo systemctl daemon-reload || systemctl daemon-reload
  [[ "$ENABLE_SERVICE" == "yes" ]] &&
    ([[ $EUID -ne 0 ]] && sudo systemctl enable --now "$SERVICE_NAME" || systemctl enable --now "$SERVICE_NAME")
}

install_udev() {
  if [[ "${INSTALL_UDEV}" != "yes" ]]; then
    echo "Skipping udev rule install (--udev no)"
    return
  fi

  echo "Installing udev rule (requires sudo)..."

  # Supported Product IDs
  PIDS=("2202" "22a1" "227e" "2206" "2258" "229e" "223a" "22a9" "227a")

  UDEV_CONTENT=""
  for pid in "${PIDS[@]}"; do
    UDEV_CONTENT+='ATTRS{idVendor}=="1038", ATTRS{idProduct}=="'"$pid"'", MODE="0660", GROUP="audio", TAG+="uaccess"
KERNEL=="hidraw*", ATTRS{idVendor}=="1038", ATTRS{idProduct}=="'"$pid"'", MODE="0660", GROUP="audio", TAG+="uaccess"
'
  done

  if [[ $EUID -ne 0 ]]; then
    echo "$UDEV_CONTENT" | sudo tee "${UDEV_RULE_PATH}" >/dev/null
    sudo udevadm control --reload
    for pid in "${PIDS[@]}"; do
        sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=1038 --attr-match=idProduct="$pid" || true
    done
  else
    echo "$UDEV_CONTENT" >"${UDEV_RULE_PATH}"
    udevadm control --reload
    for pid in "${PIDS[@]}"; do
        udevadm trigger --subsystem-match=usb --attr-match=idVendor=1038 --attr-match=idProduct="$pid" || true
    done
  fi

  echo "udev rule installed to ${UDEV_RULE_PATH}"
  echo "Make sure your user is in the 'audio' group (sudo usermod -aG audio <user>) and re-login."
}

echo "== arctis_chatmix installer =="
[[ $IS_TTY -eq 1 ]] && [[ "$(ask_yes_no "Proceed?" yes)" != "yes" ]] && exit 0

[[ "$MODE" == "user" ]] && install_user || install_system
ensure_audio_group
add_user_to_audio_group
install_udev

echo "Installation complete."
echo "If audio group was modified: LOG OUT and LOG BACK IN."

