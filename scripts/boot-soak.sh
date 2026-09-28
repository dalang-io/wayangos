#!/bin/bash
# boot-soak.sh — autonomous boot-count soak for the M7 router-kernel decision
# (docs/TODO-M7-UNBLOCK.md T3, docs/ROUTER-KERNEL-BISECT.md "Boot-count soak").
#
# The combo bisects found NO failing subset and the 1.0.20 freeze signature is
# an intermittent network stall (`gw=''`), so the decision needs boot counts:
# the suspect (block) kernel and a control (safe) kernel, each booted N>=10
# times, failure rates compared. This harness stages ONE bundle (built by
# scripts/bisect-router-opts.sh with --unattended SECS, i.e. with the safety
# net) on the device, then performs N autonomous boots: it re-stages and
# reboots over SSH while the box is reachable; a failed boot recovers by
# itself (selftest FAIL -> `wayang update --fallback` -> reboot; hard resets
# burn GRUB's 3-attempt budget). Each boot is classified from the device
# (/data/selftest.log window, running slot, /data/debug/* when present) as
#
#   up+route            selftest verdict PASS for this boot (route present)
#   up+no-route         verdict FAIL (gw='' network stall; box self-recovers)
#   watchdog-reset      armed but no verdict and a reset is evident (slot
#                       changed, or a second `armed:` line = GRUB refired)
#   unreachable>deadline no SSH within the SSH bound -> POWER-CYCLE needed
#
# Owner-measured timings on the test device (2026-09-28): the OS is up after
# a FEW SECONDS and SSH becomes active after ~120-200 s; waiting longer than
# that on a broken OS is a waste of time. The soak is sized around them:
# bundles carry `wayang.selftest=120` (the route must appear inside that
# window — a PASS/FAIL verdict is written at ~120 s uptime, i.e. BEFORE SSH
# is even up), and the harness's own liveness bound is `--ssh-bound 240`
# (headroom over 200 s). Past the bound the boot is classified broken
# immediately; verdict evidence (PASS/FAIL, or a second `armed:` line)
# resolves the boot as soon as it exists — never a fixed long deadline.
#
# Evidence is written locally (journal + per-boot files) AND on the device
# (/data/soak/<tag>/boot-NNN.txt, debug copies, soak-summary.txt), so the
# report can be rebuilt from the device alone. NEVER touches the good slot's
# content; `wayang update --from` only ever stages the idle slot.
#
# Usage:
#   scripts/boot-soak.sh --bundle FILE [--tag NAME] --boots N [options]
#   scripts/boot-soak.sh --report [--evidence DIR | --from-device] [--tag NAME]
#   scripts/boot-soak.sh --probe        one SSH contact, nothing rebooted (pre-flight)
#   scripts/boot-soak.sh --selftest     classifier tests on real-format fixtures
#   add --dry-run to any mode to print the plan and touch nothing
#
# Run options:
#   --bundle FILE       .wup to soak (bisect-router-opts.sh output; MUST be
#                       built with --unattended SECS — the soak leans on the
#                       safety net to recover failed boots by itself)
#   --tag NAME          bundle tag for logs/matrix (default: bundle filename
#                       without .wup). Use the same tag for the report.
#   --boots N           number of autonomous boots (the M7 decision: N>=10)
#   --device HOST       ssh endpoint of the device (default: root@163.128.55.3)
#   --port P --key K    ssh endpoint overrides (QEMU smoke tests)
#   --selftest-secs S   wayang.selftest= window of the bundle (default 120;
#                       re-read from the kernel cmdline after boot 1 anyway)
#   --ssh-bound S       harness liveness bound (default 240 s: SSH is active
#                       by ~120-200 s on this device, owner-measured); past
#                       it the boot is classified broken immediately
#   --recovery-margin S extra silence allowed past the SSH bound while the
#                       DESIGNED recovery (selftest FAIL -> fallback -> reboot
#                       -> good-slot SSH; or the GRUB 3-attempt budget) can
#                       still produce verdict evidence (default 120 s). The
#                       per-boot silence cap is ssh-bound + selftest-secs +
#                       this margin; a boot that resolves by evidence never
#                       waits for it.
#   --poll SECS         SSH poll cadence while a boot is in progress (15)
#   --boot-timeout S    absolute per-boot silence cap override (default:
#                       ssh-bound + selftest-secs + recovery-margin)
#   --contact-timeout S max wait for SSH before staging a boot (180)
#   --max-unreachable K stop after K consecutive unreachable>deadline boots
#                       (2) and demand a power-cycle — a stall that survives
#                       a reboot is itself data, but only a human can cycle
#   --max-consec-fail K stop early after K consecutive non-up+route boots (5;
#                       set above --boots to disable)
#   --restore-bundle F  stage this .wup at the end and reboot into it
#                       (usually the current release); without it the script
#                       only PRINTS the restore instruction
#   --selftest-host IP  write /data/etc/selftest.host (probe target) first
#   --out DIR           evidence dir (default: dist/boot-soak/<tag>-<timestamp>)
#
# Report options:
#   --evidence DIR      a run dir or the dist/boot-soak root (aggregate all
#                       run dirs found, grouped by tag)
#   --from-device       pull /data/soak/<tag>/boot-*.txt + selftest.log tail
#                       from --device instead of reading local files
#
# Env: SOAK_DEVICE, SOAK_PORT, SOAK_KEY, SOAK_EVIDENCE
# Device runs need the owner's OK (docs/TODO-M7-UNBLOCK.md, "Parallel-agent
# rules"). Exit codes: 0 ok, 1 usage, 2 soak aborted (device needs attention),
# 3 selftest (classifier) failures.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

DEVICE="${SOAK_DEVICE:-root@163.128.55.3}"
PORT="${SOAK_PORT:-}"
SSH_KEY="${SOAK_KEY:-}"
BUNDLE=""
TAG=""
BOOTS=10
SELFTEST_SECS=120
SSH_BOUND=240
RECOVERY_MARGIN=120
BOOT_TIMEOUT=""
POLL=15
CONTACT_TIMEOUT=180
MAX_UNREACHABLE=2
MAX_CONSEC_FAIL=5
RESTORE_BUNDLE=""
SELFTEST_HOST=""
EVIDENCE_ROOT="${SOAK_EVIDENCE:-$REPO_DIR/dist/boot-soak}"
EVIDENCE_DIR=""
FROM_DEVICE=0
DRY_RUN=0
MODE=""

SSH_ARGS=(-o BatchMode=yes -o ConnectTimeout=8 -o ServerAliveInterval=15 -o ServerAliveCountMax=2 -o StrictHostKeyChecking=accept-new)

usage() {
    sed -n '2,/^# Env:/p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

die() { printf 'ERROR: %s\n' "$*" >&2; exit "${2:-1}"; }
logline() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
now() { date +%s; }

sanitize_tag() { printf '%s' "$1" | tr -c 'A-Za-z0-9._+-' '_'; }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

epoch_to_date() {
    date -r "$1" "+%F %T" 2>/dev/null || date -u -d "@$1" "+%F %T" 2>/dev/null || printf '%s' "$1"
}

# ------------------------------------------------------------------ device --
dsh() { # remote command string (runs in the device's BusyBox sh; POSIX only)
    if [ "$DRY_RUN" = 1 ]; then
        printf 'DRY-RUN: ssh %s %s\n' "$DEVICE" "$*"
        return 0
    fi
    # shellcheck disable=SC2029  # the remote string is built deliberately
    ssh "${SSH_ARGS[@]}" "$DEVICE" "$@"
}

# One SSH contact. Device side is BusyBox ash (POSIX): no arrays, no bashisms.
# (The $-expressions are device-side; the string stays single-quoted on purpose.)
# shellcheck disable=SC2016
DEVICE_SNAPSHOT='
st=$(wayang status 2>&1)
slot=$(grep -o "wayang.slot=[AB]" /proc/cmdline 2>/dev/null | cut -d= -f2)
echo "SLOT=${slot:-?}"
echo "UPTIME=$(cut -d. -f1 /proc/uptime 2>/dev/null)"
echo "SFSIZE=$(stat -c %s /data/selftest.log 2>/dev/null || echo 0)"
echo "KVER=$(uname -r 2>/dev/null)"
echo "BOOTNEXT=$(printf "%s\n" "$st" | sed -n "s/^boot next: \([AB]\) .*/\1/p")"
printf "%s\n" "$st" | sed -n "1,2p" | sed "s/^/STATUS /"
'

# Parse a snapshot blob into SNAP_* globals.
parse_snapshot() {
    SNAP_SLOT="?" SNAP_UPTIME="" SNAP_SFSIZE="" SNAP_KVER="" SNAP_BOOTNEXT="" SNAP_STATUS=""
    local line
    while IFS= read -r line; do
        case "$line" in
            SLOT=*) SNAP_SLOT="${line#SLOT=}" ;;
            UPTIME=*) SNAP_UPTIME="${line#UPTIME=}" ;;
            SFSIZE=*) SNAP_SFSIZE="${line#SFSIZE=}" ;;
            KVER=*) SNAP_KVER="${line#KVER=}" ;;
            BOOTNEXT=*) SNAP_BOOTNEXT="${line#BOOTNEXT=}" ;;
            STATUS*) SNAP_STATUS="${SNAP_STATUS:+$SNAP_STATUS; }${line#STATUS }" ;;
        esac
    done <<EOF
$1
EOF
}

run_snapshot() { # -> 0 and sets SNAP_* when the device answered
    local blob
    if ! blob="$(dsh "$DEVICE_SNAPSHOT" 2>/dev/null)" || [ -z "$blob" ]; then
        return 1
    fi
    parse_snapshot "$blob"
    [ "$SNAP_SLOT" != "?" ] || return 1
    return 0
}

wait_contact() { # SECONDS -> 0 as soon as one snapshot succeeds
    local end
    end=$(( $(now) + $1 ))
    while [ "$(now)" -lt "$end" ]; do
        if run_snapshot; then return 0; fi
        sleep "$POLL"
    done
    return 1
}

# The selftest.log window for one boot: every byte appended since OFFSET.
# BusyBox tail -c +N is 1-based; verify the option worked (rc) and fall back
# to a line-based tail if this BusyBox does not support it.
# shellcheck disable=SC2016
DEVICE_WINDOW='rc=0; tail -c +$OFF /data/selftest.log 2>/dev/null || rc=$?; echo "WINDOW_RC=$rc"'
pull_window() { # OFFSET -> prints the new selftest.log lines on stdout
    local out rc
    # shellcheck disable=SC2016,SC2029  # $OFF must reach the device unexpanded
    out="$(dsh "$(printf '%s' "$DEVICE_WINDOW" | sed "s/\$OFF/$1/")" 2>/dev/null)" || out=""
    rc="$(printf '%s\n' "$out" | sed -n 's/^WINDOW_RC=//p' | head -n 1)"
    if [ "${rc:-1}" != "0" ]; then
        # shellcheck disable=SC2029
        out="$(dsh "tail -n 400 /data/selftest.log 2>/dev/null")" || out=""
    fi
    printf '%s\n' "$out" | grep -v '^WINDOW_RC=' || true
}

# ------------------------------------------------------------- classifier ---
# Classify one soak boot from its /data/selftest.log window. Pure function
# (no ssh, no files) so --selftest can exercise it on fixture logs.
#
# usage: classify_boot WINDOW_LINES FINAL_SLOT EXPECTED_SLOT DEADLINE_HIT REBOOT_SEEN CONTACT_SEEN
# sets:  C_OUTCOME C_TTF C_ATTEMPTS C_NOTES C_ARMED_SLOT C_RESOLVED
#
# CONTACT_SEEN: 1 when any SSH contact happened after the box was observed
# down. At the silence cap a boot WITH contact but no verdict is a hung
# selftest (still a broken boot -> unreachable>deadline, with a note), while
# no contact at all is the plain unreachable case.
#
# selftest.log lines look like (scripts/build-rootfs.sh, wayang-selftest):
#   2026-09-28 07:10:01 up=8s slot=B armed: 600s, target=default gateway
#   2026-09-28 07:11:30 up=97s slot=B connectivity verified ( or 1.1.1.1)
#   2026-09-28 07:20:03 up=610s slot=B PASS: reachable after 600s; slot marked good
#   2026-09-28 07:20:05 up=605s slot=B FAIL: no target reachable at the 600s deadline (host='' gw='', ever seen: 0)
#   2026-09-28 07:20:40 up=640s slot=B rebooting (reboot -f)
classify_boot() { # LINES FINAL_SLOT EXPECTED_SLOT DEADLINE_HIT
    local lines="$1" final_slot="$2" expected="$3" deadline_hit="$4" reboot_seen="$5" contact_seen="${6:-0}"
    C_OUTCOME="" C_TTF="" C_ATTEMPTS=0 C_NOTES="" C_ARMED_SLOT="" C_RESOLVED=0
    local line rest up slot verdict="" verdict_up="" markok_failed=0
    local gw="" ever="" rebooted=0
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        up="${line##*up=}"; up="${up%%s*}"
        case "$up" in ''|*[!0-9]*) up="" ;; esac
        slot="$(printf '%s' "$line" | sed -n 's/^.* slot=\([A-Z?]\) .*/\1/p')"
        [ -n "$slot" ] || continue
        rest="${line#* slot=? }"
        case "$rest" in
            armed:*)
                if [ -n "$C_ARMED_SLOT" ]; then
                    # A second armed line while we are still watching this
                    # boot = the GRUB budget already refired the bundle slot:
                    # the previous attempt reset before any verdict. Resolve
                    # immediately as watchdog-reset — do not wait for budget.
                    C_OUTCOME="watchdog-reset" C_RESOLVED=1
                    C_ATTEMPTS=$((C_ATTEMPTS + 1))
                    C_NOTES="re-armed on the same slot: previous attempt reset before a verdict"
                    return 0
                fi
                C_ARMED_SLOT="$slot"
                C_ATTEMPTS=$((C_ATTEMPTS + 1)) ;;
            PASS:*)
                if [ -n "$C_ARMED_SLOT" ] && [ -z "$verdict" ]; then
                    verdict="PASS" verdict_up="$up"
                    case "$rest" in *"mark-ok FAILED"*) markok_failed=1 ;; esac
                fi ;;
            FAIL:*)
                if [ -n "$C_ARMED_SLOT" ] && [ -z "$verdict" ]; then
                    verdict="FAIL" verdict_up="$up"
                    gw="$(printf '%s' "$rest" | sed -n "s/^.*gw='\([^']*\)'.*$/\1/p")"
                    ever="$(printf '%s' "$rest" | sed -n 's/^.*ever seen: \([0-9]*\).*$/\1/p')"
                fi ;;
            "rebooting (reboot -f)") rebooted=1 ;;
            "fallback not staged"*) C_NOTES="${C_NOTES:+$C_NOTES; }fallback-not-staged" ;;
        esac
    done <<EOF
$lines
EOF

    if [ "$verdict" = "PASS" ]; then
        C_OUTCOME="up+route" C_RESOLVED=1
        [ "$markok_failed" = 1 ] && C_NOTES="mark-ok-failed(slot-unconfirmed)"
        return 0
    fi
    if [ "$verdict" = "FAIL" ]; then
        C_OUTCOME="up+no-route" C_TTF="$verdict_up" C_RESOLVED=1
        C_NOTES="gw='$gw' ever_seen=${ever:-?}"
        [ "$rebooted" = 1 ] || C_NOTES="$C_NOTES; no-reboot-line(fallback-may-be-missing)"
        return 0
    fi
    if [ "$deadline_hit" = "1" ]; then
        C_OUTCOME="unreachable>deadline" C_RESOLVED=1
        if [ -n "$C_ARMED_SLOT" ]; then
            C_NOTES="armed x$C_ATTEMPTS, no verdict, then silent"
            [ "$contact_seen" = "1" ] && C_NOTES="$C_NOTES (SSH answered: hung selftest?)"
        elif [ "$contact_seen" = "1" ]; then
            C_NOTES="SSH answered but no armed line by the silence cap (hung selftest?)"
        else
            C_NOTES="no armed line, no contact (reboot may not have taken effect)"
        fi
        return 0
    fi
    if [ -n "$final_slot" ] && [ "$final_slot" != "?" ]; then
        if [ -n "$C_ARMED_SLOT" ] && [ "$final_slot" != "$C_ARMED_SLOT" ]; then
            C_OUTCOME="watchdog-reset" C_RESOLVED=1
            C_NOTES="reset before the ${SELFTEST_SECS}s verdict (GRUB budget path)"
            return 0
        fi
        # Only after the box was actually observed down (or its uptime went
        # backwards): right after `reboot` is issued the OLD boot still
        # answers, with an empty window — that must not classify yet.
        if [ -z "$C_ARMED_SLOT" ] && [ "$reboot_seen" = "1" ] \
            && [ -n "$expected" ] && [ "$final_slot" != "$expected" ]; then
            C_OUTCOME="watchdog-reset" C_RESOLVED=1
            C_NOTES="never armed (reset before rcS/selftest); GRUB budget path"
            return 0
        fi
    fi
    # no verdict, box still inside the window on the bundle slot: keep polling
    return 0
}

# --------------------------------------------------------------- records ----
# Internal row format (TAB-separated), one line per boot:
#   tag boot outcome ttf slot attempts ssh_back t0_epoch notes
ROWS=""

row_from_fields() { # tag boot outcome ttf slot attempts ssh_back t0 notes
    # `read` collapses adjacent TABs (IFS whitespace), so empty fields get a
    # "-" placeholder instead.
    local notes ttf slot att sb
    ttf="${4:-}"; slot="${5:-}"; att="${6:-}"; sb="${7:-}"
    [ -n "$ttf" ] || ttf="-"
    [ -n "$slot" ] || slot="-"
    [ -n "$att" ] || att="-"
    [ -n "$sb" ] || sb="-"
    notes="$(printf '%s' "${9:-}" | tr '\t\r\n' '   ')"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$1" "$2" "$3" "$ttf" "$slot" "$att" "$sb" "$8" "${notes:--}" >> "$ROWS"
}

rows_from_journal() { # FILE: skip comment lines, rows already in row format
    grep -v '^#' "$1" 2>/dev/null >> "$ROWS" || true
}

# Device record: key=value header lines + the raw selftest window as comments.
rows_from_device_blob() { # BLOB (concatenated `=== path` + record blocks)
    local tag="" boot="" outcome="" ttf="" slot="" attempts="" ssh_back="" t0="" notes=""
    local in_rec=0 line key val path
    while IFS= read -r line; do
        case "$line" in
            '=== '*)
                if [ "$in_rec" = 1 ] && [ -n "$tag" ]; then
                    row_from_fields "$tag" "$boot" "$outcome" "$ttf" "$slot" \
                        "$attempts" "$ssh_back" "$t0" "$notes"
                fi
                in_rec=0 tag="" boot="" outcome="" ttf="" slot="" attempts=""
                ssh_back="" t0="" notes=""
                path="${line#'=== '}"
                path="${path#*/data/soak/}"
                tag="${path%%/*}"
                in_rec=1 ;;
            '#'*) ;;                       # comment / selftest window copy
            '') ;;                         # blank
            *=*)
                key="${line%%=*}" val="${line#*=}"
                case "$key" in
                    boot) boot="$val" ;;
                    outcome) outcome="$val" ;;
                    ttf_s) ttf="$val" ;;
                    slot) slot="$val" ;;
                    attempts) attempts="$val" ;;
                    ssh_back_s) ssh_back="$val" ;;
                    t0_epoch) t0="$val" ;;
                    notes) notes="$val" ;;
                esac ;;
        esac
    done <<EOF
$1
EOF
    if [ "$in_rec" = 1 ] && [ -n "$tag" ]; then
        row_from_fields "$tag" "$boot" "$outcome" "$ttf" "$slot" \
            "$attempts" "$ssh_back" "$t0" "$notes"
    fi
}

render_matrix() { # ROWSFILE -> human matrix + per-tag summary on stdout
    local file="$1" tag
    if [ ! -s "$file" ]; then
        echo "(no boot records)"
        return 0
    fi
    cut -f1 "$file" | sort -u | while read -r tag; do
        echo ""
        echo "== $tag =="
        printf '%-5s %-20s %-9s %-8s %-5s %-8s %-9s %s\n' \
            boot outcome ssh_back ttf slot attempts started notes
        awk -F'\t' -v t="$tag" '$1 == t' "$file" | sort -t"$(printf '\t')" -k2,2n |
        while IFS=$'\t' read -r _t b o ttf sl att sb t0 nt; do
            local sb_show="-" ttf_show="-"
            [ "$sb" != "-" ] && sb_show="${sb}s"
            [ "$ttf" != "-" ] && ttf_show="${ttf}s"
            printf '%-5s %-20s %-9s %-8s %-5s %-8s %-9s %s\n' \
                "$b" "$o" "$sb_show" "$ttf_show" "$sl" "$att" \
                "$(epoch_to_date "$t0")" "$nt"
        done
        awk -F'\t' -v t="$tag" '
            $1 == t {
                n++
                c[$3]++
                if ($3 != "up+route") f++
                if ($4 != "" && $4 != "-") { ttfs[nf++] = $4 }
            }
            END {
                printf "total %d boots:", n
                for (o in c) printf " %s=%d", o, c[o]
                printf "\n"
                if (n > 0) printf "failure rate: %d/%d = %.1f%%\n", f + 0, n, 100 * (f + 0) / n
                if (nf > 0) {
                    s = ""
                    for (i = 0; i < nf; i++) s = s (i ? " " : "") ttfs[i]
                    print "time-to-failure (s): " s
                }
            }' "$file"
    done
    echo ""
    echo "== decision matrix (bundle x boots x outcomes) =="
    printf '%-14s %-6s %-10s %-13s %-15s %-12s %s\n' \
        bundle boots up+route up+no-route watchdog-reset unreachable "failure rate"
    cut -f1 "$file" | sort -u | while read -r tag; do
        awk -F'\t' -v t="$tag" '
            $1 == t {
                n++; c[$3]++
                if ($3 != "up+route") f++
            }
            END {
                r = (n > 0) ? sprintf("%.1f%% (%d/%d)", 100 * (f + 0) / n, f + 0, n) : "-"
                printf "%-14s %-6d %-10d %-13d %-15d %-12d %s\n",
                    t, n, c["up+route"] + 0, c["up+no-route"] + 0,
                    c["watchdog-reset"] + 0, c["unreachable>deadline"] + 0, r
            }' "$file"
    done
}

# -------------------------------------------------------------------- run ---
build_plan() { # -> plan text on stdout
    local bsha bname est_min est_max
    bsha="(file missing)"
    [ -f "$BUNDLE" ] && bsha="$(sha256_of "$BUNDLE")"
    bname="${BUNDLE:-<none>}"
    est_min=$(( (BOOTS * (SSH_BOUND + 90)) / 60 ))
    est_max=$(( (BOOTS * (SSH_BOUND + SELFTEST_SECS + RECOVERY_MARGIN + 90)) / 60 ))
    cat <<EOF
boot-soak plan
==============
mode:            ${MODE:-run}
bundle:          $bname
  sha256:        $bsha
tag:             ${TAG:-<derived from bundle>}
device:          $DEVICE
boots:           $BOOTS
selftest window: ${SELFTEST_SECS}s (the bundle MUST carry it: build with
                 bisect-router-opts.sh --unattended $SELFTEST_SECS; the actual
                 value is re-read from /proc/cmdline after boot 1)
ssh liveness
bound:           ${SSH_BOUND}s — owner-measured on this device: the OS is up
                 after a few seconds, SSH is active after ~120-200 s; past the
                 bound the boot is broken and is not waited on further
silence cap:     ${SSH_BOUND} + ${SELFTEST_SECS} + ${RECOVERY_MARGIN} = $((SSH_BOUND + SELFTEST_SECS + RECOVERY_MARGIN))s per boot — the only
                 fixed deadline; reached solely when no designed recovery
                 produces evidence (a boot that resolves never waits for it)
poll cadence:    every ${POLL}s while a boot is in progress
evidence local:  $EVIDENCE_DIR
evidence device: /data/soak/${TAG:-<tag>}/ (+ the /data/selftest.log windows)
est. wall time:  ~${est_min} min if every boot passes; worst case
                 ~${est_max} min
per boot the harness will:
  1. wait for SSH (settle, <= ${CONTACT_TIMEOUT}s) and snapshot slot/status
  2. copy /data/debug/* (if present) to /data/soak/<tag>/ for the previous boot
  3. stage: wayang update --from /data/soak.wup   (idle slot only; abort if it fails)
  4. sync; reboot, then poll SSH every ${POLL}s. A boot resolves AS SOON AS its
     evidence exists: selftest PASS/FAIL line, a second armed line (GRUB
     refire), or no SSH within the ${SSH_BOUND}s liveness bound — never a
     fixed long wait.
  5. classify from the /data/selftest.log window (byte-offset cursor) + slot:
     up+route | up+no-route | watchdog-reset | unreachable>deadline
  6. write /data/soak/<tag>/boot-NNN.txt and a local journal row
stops:
  - $MAX_UNREACHABLE consecutive unreachable>deadline -> POWER-CYCLE message, exit 2
  - $MAX_CONSEC_FAIL consecutive non-up+route boots -> stop (the data is clear)
restore:
$(if [ -n "$RESTORE_BUNDLE" ]; then
      printf '  at the end: stage %s and reboot into it (verified reachable)\n' "$RESTORE_BUNDLE"
  else
      printf '  none automatic: after the soak, restore the released image by hand\n'
      printf '  (wayang update from the channel, or --restore-bundle next time)\n'
  fi)
NOTE: device runs need the owner's OK (docs/TODO-M7-UNBLOCK.md).
EOF
}

copy_bundle_to_device() { # LOCALFILE REMOTEFILE LABEL
    logline "copying $3 to $DEVICE:$2 (no sftp-server: ssh cat)"
    dsh 'cat > '"$2"'' < "$1" || die "could not copy $3 to the device" 2
    local remote_sha local_sha
    # shellcheck disable=SC2029
    remote_sha="$(dsh "sha256sum '$2' 2>/dev/null | cut -d' ' -f1")" || die "could not hash $2 on the device" 2
    local_sha="$(sha256_of "$1")"
    [ "$remote_sha" = "$local_sha" ] || die "sha256 mismatch for $2: $remote_sha != $local_sha" 2
    logline "  sha256 ok: $local_sha"
}

run_mode() {
    [ -n "$BUNDLE" ] || die "run mode needs --bundle FILE"
    [ -f "$BUNDLE" ] || die "bundle not found: $BUNDLE"
    case "$BOOTS" in ''|*[!0-9]*|0) die "--boots needs N >= 1" ;; esac
    if [ -n "$RESTORE_BUNDLE" ] && [ ! -f "$RESTORE_BUNDLE" ]; then
        die "restore bundle not found: $RESTORE_BUNDLE"
    fi
    [ -z "$TAG" ] && TAG="$(sanitize_tag "$(basename "$BUNDLE" .wup)")"
    TAG="$(sanitize_tag "$TAG")"
    EVIDENCE_DIR="${EVIDENCE_DIR:-$EVIDENCE_ROOT/$TAG-$(date +%Y%m%d-%H%M%S)}"
    BOOT_TIMEOUT="${BOOT_TIMEOUT:-$((SSH_BOUND + SELFTEST_SECS + RECOVERY_MARGIN))}"

    if [ "$DRY_RUN" = 1 ]; then
        build_plan
        echo ""
        echo "(dry-run: nothing was touched)"
        return 0
    fi

    mkdir -p "$EVIDENCE_DIR/boots"
    build_plan > "$EVIDENCE_DIR/plan.txt"
    ROWS="$EVIDENCE_DIR/rows.tsv"
    : > "$ROWS"
    local journal="$EVIDENCE_DIR/journal.tsv"
    printf '# tag\tboot\toutcome\tttf_s\tslot\tattempts\tssh_back_s\tt0_epoch\tnotes\n' > "$journal"

    logline "soak start: tag=$TAG boots=$BOOTS bundle=$(basename "$BUNDLE")"
    dsh "mkdir -p /data/soak/'$TAG'" || die "cannot create /data/soak/$TAG on the device" 2
    {
        printf '# wayang boot-soak run header\n'
        printf 'tag=%s\nboots=%s\nselftest_secs=%s\nbundle=%s\nsha256=%s\nstarted=%s\n' \
            "$TAG" "$BOOTS" "$SELFTEST_SECS" "$(basename "$BUNDLE")" \
            "$(sha256_of "$BUNDLE")" "$(date -u +%FT%TZ)"
    } > "$EVIDENCE_DIR/run.txt.tmp"
    dsh 'cat > /data/soak/'"$TAG"'/run.txt' < "$EVIDENCE_DIR/run.txt.tmp" || true
    rm -f "$EVIDENCE_DIR/run.txt.tmp"

    if [ -n "$SELFTEST_HOST" ]; then
        dsh "mkdir -p /data/etc && echo '$SELFTEST_HOST' > /data/etc/selftest.host"
        logline "probe target pinned: /data/etc/selftest.host=$SELFTEST_HOST"
    fi

    copy_bundle_to_device "$BUNDLE" /data/soak.wup soak-bundle
    [ -z "$RESTORE_BUNDLE" ] || copy_bundle_to_device "$RESTORE_BUNDLE" /data/restore.wup restore-bundle

    # The box must be reachable before anything is staged.
    if ! wait_contact "$CONTACT_TIMEOUT"; then
        die "device did not answer SSH within ${CONTACT_TIMEOUT}s — check it before soaking" 2
    fi
    logline "device up: slot=$SNAP_SLOT uptime=${SNAP_UPTIME}s status: $SNAP_STATUS"
    local cursor=0
    cursor="${SNAP_SFSIZE:-0}"
    [ "$cursor" -ge 0 ] 2>/dev/null || cursor=0

    local n consec_unreach=0 consec_fail=0 expected="" aborted=0
    n=1
    while [ "$n" -le "$BOOTS" ]; do
        printf '\n'
        logline "=== boot $n/$BOOTS ==="

        # (1) settle: reachable + snapshot (the previous boot's recovery is done)
        if ! wait_contact "$CONTACT_TIMEOUT"; then
            logline "no contact before staging boot $n — stopping"
            printf 'aborted\tno-contact-before-boot-%s\n' "$n" >> "$journal"
            aborted=1
            break
        fi
        logline "settle: slot=$SNAP_SLOT uptime=${SNAP_UPTIME}s next=$SNAP_BOOTNEXT"

        # (2) /data/debug/* of the boot that just ended -> device + note
        if [ "$n" -gt 1 ]; then
            local dbg_out
            # shellcheck disable=SC2029
            dbg_out="$(dsh "if [ -d /data/debug ]; then mkdir -p /data/soak/'$TAG'/boot-$(printf '%03d' $((n - 1)))-debug && cp -a /data/debug/. /data/soak/'$TAG'/boot-$(printf '%03d' $((n - 1)))-debug/ && echo DEBUG_COPIED; else echo DEBUG_NONE; fi")" || dbg_out="DEBUG_FAIL"
            case "$dbg_out" in
                DEBUG_COPIED) logline "  debug saved: /data/soak/$TAG/boot-$(printf '%03d' $((n - 1)))-debug/" ;;
                DEBUG_NONE)   logline "  no /data/debug (bundle without wayang.debug?)" ;;
                *)            logline "  WARNING: could not copy /data/debug" ;;
            esac
        fi

        # (3) stage the idle slot; never reboot on a failed stage. BNEXT is
        # the fresh "boot next" read AFTER staging (the bundle slot).
        local stage_out stage_rc stage_cursor bnext
        # shellcheck disable=SC2029
        stage_out="$(dsh "wayang update --from /data/soak.wup >/tmp/soak-stage.log 2>&1; echo STAGE_RC=\$?; tail -n 3 /tmp/soak-stage.log; echo CURSOR=\$(stat -c %s /data/selftest.log 2>/dev/null || echo 0); echo BNEXT=\$(wayang status 2>/dev/null | sed -n 's/^boot next: \([AB]\) .*/\1/p')")" \
            || { logline "staging ssh failed — stopping"; printf 'aborted\tstage-ssh-failed-boot-%s\n' "$n" >> "$journal"; aborted=1; break; }
        stage_rc="$(printf '%s\n' "$stage_out" | sed -n 's/^STAGE_RC=//p')"
        stage_cursor="$(printf '%s\n' "$stage_out" | sed -n 's/^CURSOR=//p' | head -n 1)"
        bnext="$(printf '%s\n' "$stage_out" | sed -n 's/^BNEXT=//p' | head -n 1)"
        if [ "${stage_rc:-1}" != "0" ]; then
            logline "staging FAILED (rc=$stage_rc):"
            printf '%s\n' "$stage_out" | sed 's/^/    /'
            printf 'aborted\tstage-failed-boot-%s\n' "$n" >> "$journal"
            aborted=1
            break
        fi
        case "$stage_cursor" in ''|*[!0-9]*) stage_cursor="$cursor" ;; esac
        cursor="$stage_cursor"
        expected="$bnext"
        logline "staged ok; cursor=$cursor boot-next=$expected"

        # (4) reboot and poll until classified or the silence cap. Liveness is
        # the SSH bound (owner-measured: SSH active by ~120-200 s); past it
        # the boot is broken — polling continues only while the designed
        # recovery (selftest FAIL -> fallback -> reboot -> good-slot SSH, or
        # the GRUB 3-attempt budget) can still produce verdict evidence, and
        # stops the moment that evidence is read.
        dsh 'sync; reboot' || true
        local t0 deadline ssh_back="" outcome="" ttf="" slot_seen="$SNAP_SLOT" attempts=0 notes="" resolved=0
        local seen_down=0 prev_up="" window="" cmdline_checked=0 actual_secs=""
        local contact_seen=0 ssh_broken_logged=0 last_up
        t0="$(now)"
        last_up="$t0"
        deadline=$((t0 + BOOT_TIMEOUT))
        while [ "$(now)" -lt "$deadline" ]; do
            if run_snapshot; then
                # The box must be observed down (ssh refused, or its uptime
                # went backwards) before a "never armed" verdict is allowed:
                # right after `reboot` the OLD boot still answers.
                if [ "$seen_down" = 0 ]; then
                    if [ -n "$prev_up" ] && [ -n "$SNAP_UPTIME" ] \
                        && [ "$SNAP_UPTIME" -lt "$prev_up" ]; then
                        seen_down=1
                    fi
                fi
                if [ "$seen_down" = 1 ]; then
                    contact_seen=1
                fi
                prev_up="$SNAP_UPTIME"
                last_up="$(now)"
                if [ "$seen_down" = 1 ]; then
                    [ -n "$ssh_back" ] || ssh_back=$(( $(now) - t0 ))
                    ssh_broken_logged=0
                fi
                # Boot-1 safety check: the bundle must carry wayang.selftest.
                if [ "$cmdline_checked" = 0 ]; then
                    local st_arg
                    # shellcheck disable=SC2029
                    st_arg="$(dsh "grep -o 'wayang.selftest=[0-9]*' /proc/cmdline 2>/dev/null")" || st_arg=""
                    cmdline_checked=1
                    if [ -z "$st_arg" ]; then
                        logline "FATAL: the bundle kernel has no wayang.selftest= — no safety net."
                        logline "  Rebuild with bisect-router-opts.sh --unattended SECS. Recovering..."
                        # shellcheck disable=SC2029
                        dsh "wayang update --boot-other >/tmp/soak-abort.log 2>&1; rc=\$?; tail -n 2 /tmp/soak-abort.log; [ \$rc = 0 ] && { sync; reboot; }" || true
                        printf 'aborted\tbundle-has-no-selftest-boot-%s\n' "$n" >> "$journal"
                        aborted=1
                        break 2
                    fi
                    actual_secs="${st_arg#wayang.selftest=}"
                    if [ -n "$actual_secs" ] && [ "$actual_secs" != "$SELFTEST_SECS" ]; then
                        logline "  note: kernel cmdline says wayang.selftest=$actual_secs (plan assumed ${SELFTEST_SECS}s)"
                    fi
                fi
                window="$(pull_window "$cursor")"
                slot_seen="$SNAP_SLOT"
                classify_boot "$window" "$SNAP_SLOT" "$expected" 0 "$seen_down" "$contact_seen"
                if [ "$C_RESOLVED" = 1 ]; then
                    outcome="$C_OUTCOME" ttf="$C_TTF" attempts="$C_ATTEMPTS" notes="$C_NOTES"
                    resolved=1
                    break
                fi
                attempts="$C_ATTEMPTS"
            else
                seen_down=1
                local silent
                silent=$(( $(now) - last_up ))
                if [ "$ssh_broken_logged" = 0 ] && [ "$silent" -ge "$SSH_BOUND" ]; then
                    logline "no SSH for ${silent}s (bound ${SSH_BOUND}s) — boot broken;"
                    logline "  waiting only for designed-recovery evidence (cap ${BOOT_TIMEOUT}s)"
                    ssh_broken_logged=1
                fi
            fi
            sleep "$POLL"
        done
        [ "$aborted" = 0 ] || break
        if [ "$resolved" = 0 ]; then
            classify_boot "${window:-}" "${slot_seen:-?}" "$expected" 1 "$seen_down" "$contact_seen"
            outcome="$C_OUTCOME" ttf="$C_TTF" attempts="$C_ATTEMPTS" notes="$C_NOTES"
        fi

        # (5) record: local journal + boot dir, device record
        local bootdir notes_clean
        bootdir="$EVIDENCE_DIR/boots/boot-$(printf '%03d' "$n")"
        mkdir -p "$bootdir"
        notes_clean="$(printf '%s' "$notes" | tr '\t\r\n' '   ')"
        {
            printf '# wayang boot-soak record v1 (scripts/boot-soak.sh)\n'
            printf 'tag=%s\nboot=%s\noutcome=%s\nt0_epoch=%s\nssh_back_s=%s\nttf_s=%s\nattempts=%s\nslot=%s\nkernel=%s\nnotes=%s\n' \
                "$TAG" "$n" "$outcome" "$t0" "${ssh_back:--}" "${ttf:--}" \
                "${attempts:-0}" "${slot_seen:-?}" "${SNAP_KVER:-?}" "${notes_clean:--}"
            printf '# selftest window (cursor %s):\n' "$cursor"
            printf '%s\n' "$window"
        } > "$bootdir/record.txt"
        printf '%s\n' "$window" > "$bootdir/selftest-window.log"
        {
            printf 'SLOT=%s\nUPTIME=%s\nSFSIZE=%s\nKVER=%s\nBOOTNEXT=%s\nSTATUS=%s\n' \
                "$SNAP_SLOT" "$SNAP_UPTIME" "$SNAP_SFSIZE" "$SNAP_KVER" "$SNAP_BOOTNEXT" "$SNAP_STATUS"
        } > "$bootdir/snapshot.txt"
        row_from_fields "$TAG" "$n" "$outcome" "$ttf" "$slot_seen" "${attempts:-0}" \
            "$ssh_back" "$t0" "$notes_clean"
        # same row (with the tag column) into the human journal
        grep -v '^$' "$ROWS" | tail -n 1 >> "$journal"
        {
            printf '# wayang boot-soak record v1 (scripts/boot-soak.sh)\n'
            printf 'tag=%s\nboot=%s\noutcome=%s\nt0_epoch=%s\nssh_back_s=%s\nttf_s=%s\nattempts=%s\nslot=%s\nkernel=%s\nnotes=%s\n' \
                "$TAG" "$n" "$outcome" "$t0" "${ssh_back:--}" "${ttf:--}" \
                "${attempts:-0}" "${slot_seen:-?}" "${SNAP_KVER:-?}" "${notes_clean:--}"
            printf '# selftest window (cursor %s):\n' "$cursor"
            printf '%s\n' "$window"
        } | dsh 'cat > /data/soak/'"$TAG"'/boot-'"$(printf '%03d' "$n")"'.txt' || true

        logline "boot $n/$BOOTS: outcome=$outcome ssh_back=${ssh_back:-never}s ttf=${ttf:--}s slot=$slot_seen attempts=${attempts:-0} notes=${notes_clean:--}"

        # (6) stop conditions
        if [ "$outcome" = "unreachable>deadline" ]; then
            consec_unreach=$((consec_unreach + 1))
            if [ "$consec_unreach" -ge "$MAX_UNREACHABLE" ]; then
                logline "ABORT: $consec_unreach consecutive unreachable>deadline boots."
                logline "  The box needs a POWER-CYCLE (a stall survived the safety net)."
                logline "  Evidence so far: $EVIDENCE_DIR and /data/soak/$TAG/ on the device."
                aborted=1
                break
            fi
        else
            consec_unreach=0
        fi
        if [ "$outcome" != "up+route" ]; then
            consec_fail=$((consec_fail + 1))
            if [ "$consec_fail" -ge "$MAX_CONSEC_FAIL" ]; then
                logline "stop: $consec_fail consecutive failed boots — the data is clear."
                break
            fi
        else
            consec_fail=0
        fi
        n=$((n + 1))
    done

    # ---- wrap up: matrix, device summary, restore
    local summary_hdr
    summary_hdr="boot-soak summary ($(date -u +%FT%TZ))"
    [ "$aborted" = 1 ] && summary_hdr="$summary_hdr — ABORTED"
    {
        echo "$summary_hdr"
        render_matrix "$ROWS"
    } > "$EVIDENCE_DIR/summary.txt"
    cat "$EVIDENCE_DIR/summary.txt"
    dsh 'cat > /data/soak/'"$TAG"'/soak-summary.txt' < "$EVIDENCE_DIR/summary.txt" || true
    dsh "tail -c 200000 /data/selftest.log 2>/dev/null" > "$EVIDENCE_DIR/device-selftest-tail.log" 2>/dev/null || true

    if [ "$aborted" = 1 ]; then
        exit 2
    fi
    if [ -n "$RESTORE_BUNDLE" ]; then
        logline "restoring: staging /data/restore.wup and rebooting into it"
        # shellcheck disable=SC2029
        dsh "wayang update --from /data/restore.wup >/tmp/soak-restore.log 2>&1; echo RESTORE_RC=\$?; tail -n 2 /tmp/soak-restore.log" || true
        dsh 'sync; reboot' || true
        if wait_contact "$CONTACT_TIMEOUT"; then
            logline "device back: slot=$SNAP_SLOT status: $SNAP_STATUS"
        else
            logline "WARNING: device silent after the restore reboot — check /data/soak.wup is still staged"
        fi
    else
        logline "DONE. Restore the device by hand, e.g.:"
        logline "  wayang update   (stage the current release from the channel, then reboot)"
        logline "or re-run with --restore-bundle <released.wup>"
    fi
    logline "evidence: $EVIDENCE_DIR (local) and /data/soak/$TAG/ (device)"
    return 0
}

# ----------------------------------------------------------------- report ---
fetch_device_records() { # -> blob of `=== path` + record blocks on stdout
    local tag_glob="*"
    [ -n "$TAG" ] && tag_glob="$TAG"
    # shellcheck disable=SC2016  # the device must expand $f
    dsh 'for f in /data/soak/'"$tag_glob"'/boot-*.txt; do [ -f "$f" ] && { echo "=== $f"; cat "$f"; }; done' 2>/dev/null || true
}

report_mode() {
    ROWS="$(mktemp "${TMPDIR:-/tmp}/soak-rows.XXXXXX")"
    : > "$ROWS"
    if [ "$FROM_DEVICE" = 1 ]; then
        if [ "$DRY_RUN" = 1 ]; then
            echo "DRY-RUN report: would fetch from $DEVICE:"
            echo "  /data/soak/${TAG:-*}/boot-*.txt   (per-boot records)"
            echo "  /data/selftest.log tail           (corroboration)"
            echo "  /data/boots.log                   (QEMU test hook, when present)"
            return 0
        fi
        rows_from_device_blob "$(fetch_device_records)"
        dsh 'tail -n 60 /data/boots.log 2>/dev/null' > "${ROWS}.boots.log" 2>/dev/null || true
        if [ -s "${ROWS}.boots.log" ]; then
            local qemu_boots
            qemu_boots="$(grep -c 'boot wayang.slot=' "${ROWS}.boots.log" || true)"
            echo "(device /data/boots.log: $qemu_boots recorded boots — QEMU test hook)"
        fi
    else
        local root="$EVIDENCE_DIR"
        [ -n "$root" ] || root="$EVIDENCE_ROOT"
        if [ "$DRY_RUN" = 1 ]; then
            echo "DRY-RUN report: would read journals under $root:"
            if [ -f "$root" ]; then
                echo "  $root (single journal)"
            else
                find "$root" -name journal.tsv 2>/dev/null | sort || true
                [ -d "$root" ] || echo "  (directory does not exist yet)"
            fi
            return 0
        fi
        if [ -f "$root" ]; then
            rows_from_journal "$root"
        elif [ -d "$root" ] && [ -f "$root/journal.tsv" ]; then
            rows_from_journal "$root/journal.tsv"
        elif [ -d "$root" ]; then
            local j
            find "$root" -name journal.tsv 2>/dev/null | sort | while read -r j; do
                rows_from_journal "$j"
            done
        else
            die "evidence not found: $root (run the soak first, or use --from-device)"
        fi
    fi
    if [ -n "$TAG" ]; then
        local filtered
        filtered="$(mktemp "${TMPDIR:-/tmp}/soak-rows.XXXXXX")"
        awk -F'\t' -v t="$TAG" '$1 == t' "$ROWS" > "$filtered" || true
        mv "$filtered" "$ROWS"
    fi
    render_matrix "$ROWS"
    rm -f "$ROWS" "${ROWS}.boots.log"
}

# --------------------------------------------------------------- selftest ---
# Classifier fixtures using the EXACT real log formats from
# scripts/build-rootfs.sh (wayang-selftest) and scripts/test-selftest-qemu.sh.
ASSERT_FAILS=0
assert_eq() { # actual expected label
    if [ "$2" = "$3" ]; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FAIL %s: got [%s] want [%s]\n' "$1" "$2" "$3"
        ASSERT_FAILS=$((ASSERT_FAILS + 1))
    fi
}
assert_contains() { # haystack needle label
    case "$1" in *"$2"*) printf 'ok   %s\n' "$3" ;;
        *) printf 'FAIL %s: [%s] does not contain [%s]\n' "$3" "$1" "$2"
           ASSERT_FAILS=$((ASSERT_FAILS + 1)) ;;
    esac
}

selftest_mode() {
    local armB conn pass pass_bad fail fail_late reb armA
    armB="2026-09-28 07:10:01 up=8s slot=B armed: 120s, target=default gateway"
    conn="2026-09-28 07:11:30 up=97s slot=B connectivity verified ( or 1.1.1.1)"
    pass="2026-09-28 07:12:03 up=126s slot=B PASS: reachable after 120s; slot marked good"
    pass_bad="2026-09-28 07:12:03 up=126s slot=B PASS: reachable after 120s, but mark-ok FAILED (slot not confirmed)"
    fail="2026-09-28 07:12:05 up=124s slot=B FAIL: no target reachable at the 120s deadline (host='' gw='', ever seen: 0)"
    fail_late="2026-09-28 07:12:05 up=124s slot=B FAIL: no target reachable at the 120s deadline (host='' gw='', ever seen: 1)"
    reb="2026-09-28 07:12:40 up=130s slot=B rebooting (reboot -f)"
    armA="2026-09-28 07:09:58 up=9s slot=A armed: 120s, target=default gateway"

    printf '%s\n' "--- classifier fixtures (real selftest.log formats) ---"

    classify_boot "$armB
$conn
$pass" "B" "B" 0 1 1
    assert_eq "pass -> up+route" "$C_OUTCOME" "up+route"
    assert_eq "pass -> attempts" "$C_ATTEMPTS" "1"
    assert_eq "pass -> ttf empty" "$C_TTF" ""
    assert_eq "pass -> resolved" "$C_RESOLVED" "1"

    classify_boot "$armB
$pass_bad" "B" "B" 0 1 1
    assert_eq "pass(mark-ok failed) -> up+route" "$C_OUTCOME" "up+route"
    assert_contains "$C_NOTES" "mark-ok-failed" "pass(mark-ok failed) -> note"

    classify_boot "$armB
$fail
$reb" "A" "B" 0 1 1
    assert_eq "fail(gw='') -> up+no-route" "$C_OUTCOME" "up+no-route"
    assert_eq "fail -> ttf=124" "$C_TTF" "124"
    assert_contains "$C_NOTES" "gw=''" "fail -> gw='' evidence"

    classify_boot "$armB
$conn
$fail_late
$reb" "A" "B" 0 1 1
    assert_eq "late stall -> up+no-route" "$C_OUTCOME" "up+no-route"
    assert_contains "$C_NOTES" "ever_seen=1" "late stall -> ever_seen=1 (stall after early connectivity)"

    classify_boot "$armB
$fail" "A" "B" 0 1 1
    assert_contains "$C_NOTES" "no-reboot-line" "fail without reboot line -> fallback note"

    classify_boot "$armB
$armB" "A" "B" 0 1 1
    assert_eq "armed x2, back on A -> watchdog-reset" "$C_OUTCOME" "watchdog-reset"
    assert_eq "watchdog-reset -> attempts=2" "$C_ATTEMPTS" "2"

    # A second armed line while the box is STILL on the bundle slot is
    # already the reset evidence (GRUB refired): resolve immediately, do not
    # wait for the budget to exhaust.
    classify_boot "$armB
$armB" "B" "B" 0 1 1
    assert_eq "armed x2, still on B -> watchdog-reset now" "$C_OUTCOME" "watchdog-reset"
    assert_contains "$C_NOTES" "re-armed" "armed x2 same slot -> re-arm note"

    classify_boot "" "A" "B" 0 1 1
    assert_eq "never armed, back on A -> watchdog-reset" "$C_OUTCOME" "watchdog-reset"
    assert_eq "never armed -> attempts=0" "$C_ATTEMPTS" "0"

    # Right after `reboot` is issued the OLD boot still answers (seen_down=0):
    # an empty window on the old slot must NOT classify as watchdog-reset.
    classify_boot "" "A" "B" 0 0 0
    assert_eq "old boot still answering -> unresolved" "$C_RESOLVED" "0"

    classify_boot "$armB" "B" "B" 0 1 1
    assert_eq "in window, still on B -> unresolved" "$C_RESOLVED" "0"

    # Silence cap (the only fixed deadline) with NO post-down contact.
    classify_boot "" "?" "B" 1 1 0
    assert_eq "cap, no contact -> unreachable" "$C_OUTCOME" "unreachable>deadline"

    # Silence cap WITH post-down contact and an armed line but no verdict:
    # the selftest never wrote one — still a broken boot, with a note.
    classify_boot "$armB" "B" "B" 1 1 1
    assert_eq "cap, reachable, armed, no verdict -> unreachable" "$C_OUTCOME" "unreachable>deadline"
    assert_contains "$C_NOTES" "hung selftest" "cap, reachable no verdict -> hung-selftest note"

    # A straggler verdict from the PREVIOUS boot (written between the cursor
    # read and the reboot) must not classify the new boot: only a verdict
    # AFTER this boot's armed line counts.
    classify_boot "$pass
$armB" "B" "B" 0 1 1
    assert_eq "straggler PASS before armed -> unresolved" "$C_RESOLVED" "0"

    # Good-slot armed line (boot on A) followed by verdict on A: slot parsing.
    classify_boot "$armA
$pass" "?" "" 0 1 1
    assert_eq "slot-A boot PASS -> up+route" "$C_OUTCOME" "up+route"

    printf '%s\n' "--- device record parser ---"
    local rec
    rec="$(cat <<'EOF'
=== /data/soak/soak-block/boot-003.txt
# wayang boot-soak record v1 (scripts/boot-soak.sh)
tag=soak-block
boot=3
outcome=up+no-route
t0_epoch=1759000000
ssh_back_s=41
ttf_s=605
attempts=1
slot=B
kernel=7.2.7
notes=gw='' ever_seen=0
# selftest window (cursor 123):
2026-09-28 07:20:05 up=605s slot=B FAIL: no target reachable at the 600s deadline (host='' gw='', ever seen: 0)
EOF
)"
    ROWS="$(mktemp "${TMPDIR:-/tmp}/soak-rows.XXXXXX")"
    : > "$ROWS"
    rows_from_device_blob "$rec"
    assert_eq "device record -> 1 row" "$(wc -l < "$ROWS" | tr -d ' ')" "1"
    assert_eq "device record -> outcome" "$(awk -F'\t' '{print $3}' "$ROWS")" "up+no-route"
    assert_eq "device record -> tag" "$(awk -F'\t' '{print $1}' "$ROWS")" "soak-block"
    assert_eq "device record -> ttf" "$(awk -F'\t' '{print $4}' "$ROWS")" "605"

    printf '%s\n' "--- boots.log (QEMU test hook) parser ---"
    local boots_log qemu_count
    boots_log='2026-09-28 07:10:00 boot wayang.slot=A wdt=yes
2026-09-28 07:20:00 boot wayang.slot=B wdt=yes
2026-09-28 07:21:10 boot wayang.slot=B wdt=yes
2026-09-28 07:30:00 boot wayang.slot=A wdt=yes'
    qemu_count="$(printf '%s\n' "$boots_log" | grep -c 'boot wayang.slot=' || true)"
    assert_eq "boots.log -> 4 boots" "$qemu_count" "4"

    printf '%s\n' "--- report math ---"
    ROWS="$(mktemp "${TMPDIR:-/tmp}/soak-rows.XXXXXX")"
    : > "$ROWS"
    local i
    {
        for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
            case "$i" in
                4)  row_from_fields soak-block "$i" "up+no-route" 605 B 1 41 1759000000 "gw=''" ;;
                9)  row_from_fields soak-block "$i" "watchdog-reset" "" B 2 33 1759000300 "reset" ;;
                12) row_from_fields soak-block "$i" "up+no-route" 603 B 1 44 1759000600 "gw=''" ;;
                *)  row_from_fields soak-block "$i" "up+route" "" B 1 38 1759000400 "-" ;;
            esac
        done
        for i in 1 2 3 4; do
            row_from_fields soak-safe "$i" "up+route" "" A 1 40 1759001000 "-"
        done
    }
    local matrix
    matrix="$(render_matrix "$ROWS")"
    assert_contains "$matrix" "failure rate: 3/12 = 25.0%" "soak-block rate 3/12"
    assert_contains "$matrix" "failure rate: 0/4 = 0.0%" "soak-safe rate 0/4"
    assert_contains "$matrix" "soak-block" "matrix lists soak-block"
    local med
    med="$(printf '605\n607\n' | sort -n | awk '{a[NR]=$1} END { if (NR%2) print a[(NR+1)/2]; else print int((a[NR/2]+a[NR/2+1])/2) }')"
    assert_eq "median TTF 605,607 -> 606" "$med" "606"
    rm -f "$ROWS"

    printf '%s\n' "--- done ---"
    if [ "$ASSERT_FAILS" -gt 0 ]; then
        printf '%d assertion(s) FAILED\n' "$ASSERT_FAILS"
        exit 3
    fi
    echo "ALL CLASSIFIER TESTS PASSED"
}

# ------------------------------------------------------------------- probe --
# One SSH contact, nothing rebooted: verify the device plumbing (snapshot
# parse, tail -c window cursor) before trusting it with an overnight soak.
probe_mode() {
    if [ "$DRY_RUN" = 1 ]; then
        echo "DRY-RUN probe: would contact $DEVICE and print one snapshot plus"
        echo "  a mid-file /data/selftest.log window (verifies BusyBox tail -c +N)"
        echo "  and the tags found under /data/soak/."
        return 0
    fi
    wait_contact "$CONTACT_TIMEOUT" || die "no SSH contact with $DEVICE" 2
    echo "snapshot:"
    echo "  slot=$SNAP_SLOT uptime=${SNAP_UPTIME}s selftest.log=${SNAP_SFSIZE}B kernel=$SNAP_KVER"
    echo "  boot-next=$SNAP_BOOTNEXT"
    echo "  status: $SNAP_STATUS"
    local half window_bytes tags
    half=$(( ${SNAP_SFSIZE:-0} / 2 ))
    window="$(pull_window "$half")"
    window_bytes="$(printf '%s' "$window" | wc -c | tr -d ' ')"
    echo "window: tail -c +$((half + 1)) returned ${window_bytes}B (expect ~half of ${SNAP_SFSIZE:-0})"
    # shellcheck disable=SC2016,SC2029
    tags="$(dsh 'ls -d /data/soak/* 2>/dev/null | xargs -n1 basename 2>/dev/null; true' 2>/dev/null)" || tags=""
    echo "device soak tags: ${tags:-none}"
    local bl
    # shellcheck disable=SC2029
    bl="$(dsh 'tail -n 5 /data/boots.log 2>/dev/null; true' 2>/dev/null)" || bl=""
    if [ -n "$bl" ]; then
        echo "boots.log tail (QEMU test hook):"
        printf '%s\n' "$bl" | sed 's/^/  /'
    fi
    echo "probe OK"
}

# -------------------------------------------------------------------- main --
while [ $# -gt 0 ]; do
    case "$1" in
        --bundle)
            [ $# -ge 2 ] || { echo "ERROR: --bundle needs a file" >&2; exit 1; }
            BUNDLE="$2"; shift ;;
        --tag)
            [ $# -ge 2 ] || { echo "ERROR: --tag needs a name" >&2; exit 1; }
            TAG="$2"; shift ;;
        --boots)
            [ $# -ge 2 ] || { echo "ERROR: --boots needs N" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --boots needs N" >&2; exit 1 ;; esac
            BOOTS="$2"; shift ;;
        --device)
            [ $# -ge 2 ] || { echo "ERROR: --device needs a host" >&2; exit 1; }
            DEVICE="$2"; shift ;;
        --port)
            [ $# -ge 2 ] || { echo "ERROR: --port needs a number" >&2; exit 1; }
            PORT="$2"; shift ;;
        --key)
            [ $# -ge 2 ] || { echo "ERROR: --key needs a file" >&2; exit 1; }
            SSH_KEY="$2"; shift ;;
        --selftest-secs)
            [ $# -ge 2 ] || { echo "ERROR: --selftest-secs needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --selftest-secs needs SECONDS" >&2; exit 1 ;; esac
            SELFTEST_SECS="$2"; shift ;;
        --poll)
            [ $# -ge 2 ] || { echo "ERROR: --poll needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --poll needs SECONDS" >&2; exit 1 ;; esac
            POLL="$2"; shift ;;
        --ssh-bound)
            [ $# -ge 2 ] || { echo "ERROR: --ssh-bound needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --ssh-bound needs SECONDS" >&2; exit 1 ;; esac
            SSH_BOUND="$2"; shift ;;
        --recovery-margin)
            [ $# -ge 2 ] || { echo "ERROR: --recovery-margin needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --recovery-margin needs SECONDS" >&2; exit 1 ;; esac
            RECOVERY_MARGIN="$2"; shift ;;
        --boot-timeout)
            [ $# -ge 2 ] || { echo "ERROR: --boot-timeout needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --boot-timeout needs SECONDS" >&2; exit 1 ;; esac
            BOOT_TIMEOUT="$2"; shift ;;
        --contact-timeout)
            [ $# -ge 2 ] || { echo "ERROR: --contact-timeout needs SECONDS" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --contact-timeout needs SECONDS" >&2; exit 1 ;; esac
            CONTACT_TIMEOUT="$2"; shift ;;
        --max-unreachable)
            [ $# -ge 2 ] || { echo "ERROR: --max-unreachable needs K" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --max-unreachable needs K" >&2; exit 1 ;; esac
            MAX_UNREACHABLE="$2"; shift ;;
        --max-consec-fail)
            [ $# -ge 2 ] || { echo "ERROR: --max-consec-fail needs K" >&2; exit 1; }
            case "$2" in ''|*[!0-9]*) echo "ERROR: --max-consec-fail needs K" >&2; exit 1 ;; esac
            MAX_CONSEC_FAIL="$2"; shift ;;
        --restore-bundle)
            [ $# -ge 2 ] || { echo "ERROR: --restore-bundle needs a file" >&2; exit 1; }
            RESTORE_BUNDLE="$2"; shift ;;
        --selftest-host)
            [ $# -ge 2 ] || { echo "ERROR: --selftest-host needs an IP" >&2; exit 1; }
            SELFTEST_HOST="$2"; shift ;;
        --out)
            [ $# -ge 2 ] || { echo "ERROR: --out needs a directory" >&2; exit 1; }
            EVIDENCE_DIR="$2"; shift ;;
        --evidence)
            [ $# -ge 2 ] || { echo "ERROR: --evidence needs a directory" >&2; exit 1; }
            EVIDENCE_DIR="$2"; shift ;;
        --report) MODE="report" ;;
        --probe) MODE="probe" ;;
        --from-device) FROM_DEVICE=1; MODE="report" ;;
        --selftest) MODE="selftest" ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help) usage 0 ;;
        --*) echo "ERROR: unknown option: $1" >&2; usage 1 ;;
        *) echo "ERROR: unexpected argument: $1" >&2; usage 1 ;;
    esac
    shift
done

[ -n "$PORT" ] && SSH_ARGS+=(-p "$PORT")
[ -n "$SSH_KEY" ] && SSH_ARGS+=(-i "$SSH_KEY")

case "$MODE" in
    selftest)
        selftest_mode
        ;;
    report)
        report_mode
        ;;
    probe)
        probe_mode
        ;;
    run|"")
        run_mode
        ;;
esac
