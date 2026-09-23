#!/bin/bash
# Exercise dcheck against a synthetic /sys + /proc fixture.
#
# This lets enumeration and reporting be tested on any host (no real disks,
# no Linux required).
#
# Usage:
#   ./scripts/test-dcheck.sh                         # run assertions
#   CREATE_ONLY=1 ./scripts/test-dcheck.sh           # create a reusable fixture
#   DCHECK_FIXTURE_DIR=/tmp/x CREATE_ONLY=1 ./scripts/test-dcheck.sh
#   DCHECK_BIN=/path/to/dcheck ./scripts/test-dcheck.sh
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEBUG_BIN="${CARGO_TARGET_DIR:-$REPO_DIR/dcheck/target}/debug/dcheck"

if [ -n "${CREATE_ONLY:-}" ]; then
    FAKE="${DCHECK_FIXTURE_DIR:-/tmp/dcheck-fixture}"
    rm -rf "$FAKE"
else
    FAKE="$(mktemp -d)"
    trap 'rm -rf "$FAKE"' EXIT
fi

# ---- sysfs / proc fixture ------------------------------------------------
mkdir -p "$FAKE/sys/block" "$FAKE/sys/class/nvme/nvme0" "$FAKE/proc/self"
mkdir -p "$FAKE/sys/devices/pci0000:00/nvme/nvme0/nvme0n1"
mkdir -p "$FAKE/sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0"
mkdir -p "$FAKE/sys/devices/pci0000:00/ata2/host1/target1:0:0/1:0:0:0"
mkdir -p "$FAKE/sys/devices/pci0000:00/usb1/1-1/1-1:1.0/host6/target6:0:0/6:0:0:0"
mkdir -p "$FAKE/sys/devices/platform/soc/mmc_host/mmc0/mmc0:0001"

# $1=name $2=size_sectors $3=rotational $4=removable $5=dev $6=device-target
block() {
    local base="$FAKE/sys/block/$1"
    mkdir -p "$base/queue"
    printf '%s\n' "$2" > "$base/size"
    printf '%s\n' "$3" > "$base/queue/rotational"
    printf '512\n' > "$base/queue/logical_block_size"
    printf '%s\n' "$4" > "$base/removable"
    printf '%s\n' "$5" > "$base/dev"
    ln -s "$6" "$base/device"
}

# $1=disk $2=partition $3=size_sectors $4=dev
part() {
    local base="$FAKE/sys/block/$1/$2"
    mkdir -p "$base"
    printf '%s\n' "$3" > "$base/size"
    printf '%s\n' "$4" > "$base/dev"
}

# NVMe SSD 500GB, root partition 100GB, unmounted 300GB
block nvme0n1 976562500 0 0 259:0 ../../devices/pci0000:00/nvme/nvme0/nvme0n1
part nvme0n1 nvme0n1p1 195312500 259:1
part nvme0n1 nvme0n1p2 585937500 259:2
printf 'Samsung SSD 980 500GB\n' > "$FAKE/sys/class/nvme/nvme0/model"
printf 'S5GXNX0R123456\n'          > "$FAKE/sys/class/nvme/nvme0/serial"
printf '2B4QFXO7\n'                > "$FAKE/sys/class/nvme/nvme0/firmware_rev"

# SATA SSD 240GB, /boot mounted, rest unmounted
block sda 468750000 0 0 8:0 ../../devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0
part sda sda1 1953125 8:1
part sda sda2 390625000 8:2
printf 'ATA     \n'             > "$FAKE/sys/block/sda/device/vendor"
printf 'KINGSTON SA400S37\n'    > "$FAKE/sys/block/sda/device/model"
printf 'R0105A\n'               > "$FAKE/sys/block/sda/device/rev"
printf 'ABC123\n'               > "$FAKE/sys/block/sda/device/serial"

# SATA HDD 1TB, one unmounted partition
block sdb 1953125000 1 0 8:16 ../../devices/pci0000:00/ata2/host1/target1:0:0/1:0:0:0
part sdb sdb1 1953125000 8:17
printf 'ATA     \n'             > "$FAKE/sys/block/sdb/device/vendor"
printf 'WDC WD10SPZX-00Z10T0\n' > "$FAKE/sys/block/sdb/device/model"

# USB flash 16GB, removable, no partitions
block sdc 31250000 1 1 8:32 ../../devices/pci0000:00/usb1/1-1/1-1:1.0/host6/target6:0:0/6:0:0:0
printf 'Generic \n'    > "$FAKE/sys/block/sdc/device/vendor"
printf 'Flash Disk\n'  > "$FAKE/sys/block/sdc/device/model"

# eMMC/SD 32GB, one mounted partition
block mmcblk0 62500000 0 0 179:0 ../../devices/platform/soc/mmc_host/mmc0/mmc0:0001
part mmcblk0 mmcblk0p1 62500000 179:1

# These must be ignored
mkdir -p "$FAKE/sys/block/loop0" "$FAKE/sys/block/dm-0" "$FAKE/sys/block/sr0"

cat > "$FAKE/proc/self/mountinfo" <<'EOF'
1 0 259:1 / / rw,relatime - ext4 /dev/nvme0n1p1 rw
2 1 8:1 / /boot rw,relatime - vfat /dev/sda1 rw
3 1 179:1 / /media/card rw,relatime - ext4 /dev/mmcblk0p1 rw
EOF

# ---- assertions ----------------------------------------------------------
pass=0
fail=0
contains() {
    local haystack="$1" needle="$2" label="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        echo "  ok   : $label"
        pass=$((pass + 1))
    else
        echo "  FAIL : $label (missing: $needle)"
        fail=$((fail + 1))
    fi
}
absent() {
    local haystack="$1" needle="$2" label="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        echo "  FAIL : $label (unexpected: $needle)"
        fail=$((fail + 1))
    else
        echo "  ok   : $label"
        pass=$((pass + 1))
    fi
}

# ---- fixture ready; either print usage or run assertions ----------------
if [ -n "${CREATE_ONLY:-}" ]; then
    (cd "$REPO_DIR/dcheck" && cargo build --quiet)
    cat <<EOF
Fixture created at: $FAKE

Try it manually:
  (cd "$REPO_DIR/dcheck" && cargo build)
  DCHECK_SYS_ROOT=$FAKE $DEBUG_BIN                       # interactive menu
  DCHECK_SYS_ROOT=$FAKE $DEBUG_BIN storage               # device list + picker
  DCHECK_SYS_ROOT=$FAKE $DEBUG_BIN storage /dev/nvme0n1  # one device report
  DCHECK_SYS_ROOT=$FAKE $DEBUG_BIN storage /dev/sdb
EOF
    exit 0
fi

if [ -n "${DCHECK_BIN:-}" ]; then
    BIN="$DCHECK_BIN"
else
    (cd "$REPO_DIR/dcheck" && cargo build --quiet)
    BIN="$DEBUG_BIN"
fi
[ -x "$BIN" ] || { echo "ERROR: dcheck binary not found: $BIN" >&2; exit 1; }

export DCHECK_SYS_ROOT="$FAKE"
# Each assertion must see the fixture as it is now, never a cached read.
export DCHECK_NO_CACHE=1

echo "=== dcheck storage (list) ==="
LIST="$("$BIN" storage 2>&1 || true)"
printf '%s\n' "$LIST"
echo

contains "$LIST" "/dev/nvme0n1"                       "NVMe listed"
contains "$LIST" "Samsung SSD 980 500GB"              "NVMe model"
contains "$LIST" "500 GB"                             "NVMe capacity"
contains "$LIST" "KINGSTON SA400S37"                  "SATA SSD model"
contains "$LIST" "WDC WD10SPZX-00Z10T0"               "SATA HDD model"
contains "$LIST" "/dev/sdc"                           "USB device listed"
contains "$LIST" "/dev/mmcblk0"                       "MMC device listed"
contains "$LIST" "NVMe"                               "NVMe type/bus label"
contains "$LIST" "HDD"                                "HDD type label"
absent   "$LIST" "loop0"                              "loop devices ignored"
absent   "$LIST" "dm-0"                               "device-mapper ignored"
absent   "$LIST" "sr0"                                "optical ignored"

echo
echo "=== dcheck storage /dev/sdb (report) ==="
REP="$("$BIN" storage /dev/sdb 2>&1 || true)"
printf '%s\n' "$REP"
echo

contains "$REP" "WDC WD10SPZX-00Z10T0"   "HDD model in report"
contains "$REP" "HDD"                    "HDD classified"
contains "$REP" "SATA"                   "SATA bus detected"
contains "$REP" "1.0 TB"                 "HDD capacity"
contains "$REP" "/dev/sdb1"              "partition listed"
contains "$REP" "not mounted"            "unmounted partition shown"

echo
echo "=== dcheck storage nvme0n1 (report, by kernel name) ==="
REP2="$("$BIN" storage nvme0n1 2>&1 || true)"
printf '%s\n' "$REP2"
echo

contains "$REP2" "Samsung SSD 980 500GB" "NVMe model in report"
contains "$REP2" "S5GXNX0R123456"        "NVMe serial"
contains "$REP2" "2B4QFXO7"              "NVMe firmware"
contains "$REP2" "NVMe"                  "NVMe classified"
contains "$REP2" "/dev/nvme0n1p1"        "NVMe partition"
contains "$REP2" "ext4"                  "filesystem from mountinfo"
contains "$REP2" "/"                     "mountpoint joined"

echo
echo "=== dcheck storage /dev/sda with SMART (sample JSON) ==="
SMART_JSON="$REPO_DIR/dcheck/testdata/smart-sata-sample.json"
REP3="$(DCHECK_SMART_JSON="$SMART_JSON" "$BIN" storage /dev/sda 2>&1 || true)"
printf '%s\n' "$REP3"
echo

contains "$REP3" "Samsung SSD 870 EVO 500GB" "SMART model in report"
contains "$REP3" "SMART status : passed"      "SMART status parsed"
contains "$REP3" "Temperature  : 33"          "SMART temperature parsed"
contains "$REP3" "Link speed   : 6.0 Gb/s (SATA 3.2)" "interface speed parsed"
contains "$REP3" "Rated TBW"                  "rated endurance looked up"
contains "$REP3" "Confidence   : medium"      "confidence computed"
contains "$REP3" "Life left    : ~"           "remaining-life estimate"
contains "$REP3" "Estimate from: host writes vs rated endurance" "estimate basis shown"

echo
echo "=== dcheck storage (unknown device) ==="
if "$BIN" storage /dev/nope >/dev/null 2>&1; then
    echo "  FAIL : unknown device should return non-zero"
    fail=$((fail + 1))
else
    echo "  ok   : unknown device returns non-zero"
    pass=$((pass + 1))
fi

echo
echo "=== failed SATA port from the kernel log (DCHECK_KMSG) ==="
KMSG="$(mktemp)"
cat > "$KMSG" <<'KMSG_EOF'
ata5: SATA max UDMA/133 abar m2048@0xfb616000 port 0xfb616100 irq 38
ata5: link is slow to respond, please be patient (ready=0)
ata5: limiting SATA link speed to 3.0 Gbps
ata5: hardreset failed
ata5: reset failed, giving up
KMSG_EOF
code=0
CHK="$(DCHECK_KMSG="$KMSG" "$BIN" check 2>&1)" || code=$?
printf '%s\n' "$CHK"
contains "$CHK" "ata5"                      "dead port listed by check"
contains "$CHK" "never became ready"        "reason shown"
contains "$CHK" "REPLACE"                   "verdict is REPLACE"
if [ "$code" -eq 3 ]; then echo "  ok   : check exits 3"; pass=$((pass + 1)); else echo "  FAIL : check exit $code (want 3)"; fail=$((fail + 1)); fi
PREP="$(DCHECK_KMSG="$KMSG" "$BIN" storage ata5 2>&1 || true)"
contains "$PREP" "SATA port"                "port report"
contains "$PREP" "never answered IDENTIFY"  "port report explains"
rm -f "$KMSG"

echo
echo "=== RESULT: $pass passed, $fail failed ==="
[ "$fail" -eq 0 ]
