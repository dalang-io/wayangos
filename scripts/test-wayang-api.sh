#!/bin/bash
# Real-binary check of `wayang api`: tokens/scopes, writes + audit + replay, SSE,
# read-only default, rate limit, flag guard rails, TLS and mutual TLS with
# openssl-made certificates. Run on the builder against a musl build:
#   bash scripts/test-wayang-api.sh   (expects /tmp/wayangos-dev/wayang/target/x86_64-unknown-linux-musl/release/wayang)
# Everything lives in a temp WAYANG_ROOT; nothing outside it is touched.
set -u
B=/tmp/wayangos-dev/wayang/target/x86_64-unknown-linux-musl/release/wayang
T=/tmp/wayang-api-real.$$; mkdir -p $T/root/data/etc/wayangi $T/root/root/.ssh $T/root/etc/wayang $T/root/boot/grub $T/tls
export WAYANG_ROOT=$T/root
echo 1.0.30 > $T/root/etc/wayang/version
pass=0; fail=0
check(){ if eval "$2"; then echo "PASS  $1"; pass=$((pass+1)); else echo "FAIL  $1"; fail=$((fail+1)); fi; }
TF=$WAYANG_ROOT/data/etc/wayangi/api-token
ADMIN=$($B api --gen-token --token-file $TF --scope admin --label ops)
RO=$($B api --gen-token --token-file $TF --scope ro --label dash --append)
RW=$($B api --gen-token --token-file $TF --scope rw --label ci --append)
check "gen-token: three tokens, file 0600" '[ $(wc -l < $TF) = 3 ] && [ "$(stat -c %a $TF)" = 600 ]'
P=18632
$B api --listen 127.0.0.1:$P --rw --no-rate-limit > $T/api.log 2>&1 &
S=$!
trap 'kill $S $S2 $S3 2>/dev/null; rm -rf $T' EXIT
for _ in $(seq 1 30); do curl -s -o /dev/null http://127.0.0.1:$P/v1/health && break; sleep 0.2; done
code(){ curl -s -o $T/body -w '%{http_code}' "$@"; }
H="http://127.0.0.1:$P"
check "health is open and shows rw/tls" '[ "$(code $H/v1/health)" = 200 ] && grep -q "\"rw\":true" $T/body && grep -q "\"tls\":false" $T/body'
check "status without a token: 401" '[ "$(code $H/v1/status)" = 401 ]'
check "ro token reads ssh-keys" '[ "$(code -H "Authorization: Bearer $RO" $H/v1/ssh-keys)" = 200 ]'
check "ro token may not add a key (403 forbidden)" '[ "$(code -X POST -H "Authorization: Bearer $RO" --data-binary "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys a@b" $H/v1/ssh-keys)" = 403 ] && grep -q forbidden $T/body'
check "rw token may not either (admin only)" '[ "$(code -X POST -H "Authorization: Bearer $RW" --data-binary x $H/v1/ssh-keys)" = 403 ]'
KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys alice@box"
check "admin adds a key" '[ "$(code -X POST -H "Authorization: Bearer $ADMIN" --data-binary "$KEY" $H/v1/ssh-keys)" = 200 ] && grep -q "SHA256:" $T/body && grep -q alice $WAYANG_ROOT/data/etc/ssh/authorized_keys'
check "the key list shows a fingerprint and no key material" '[ "$(code -H "Authorization: Bearer $RO" $H/v1/ssh-keys)" = 200 ] && grep -q "SHA256:" $T/body && ! grep -q "AAAAC3Nza" $T/body'
check "reset without the second key: 400, nothing removed" '[ "$(code -X POST -H "Authorization: Bearer $ADMIN" $H/v1/reset)" = 400 ]'
JSON='{"router_toml":"[[interface]]\nname = \"lan\"\n","fw_toml":"[[zone]]\nname = \"lan\"\n"}'
check "edge bundle as JSON: installed, applied=false" '[ "$(code -X POST -H "Authorization: Bearer $ADMIN" -H "Idempotency-Key: b1" --data-binary "$JSON" $H/v1/edgerouter/apply)" = 200 ] && grep -q "\"applied\":false" $T/body && [ -f $WAYANG_ROOT/data/etc/fw/config.toml ]'
check "the same Idempotency-Key replays" '[ "$(code -D $T/hdr -X POST -H "Authorization: Bearer $ADMIN" -H "Idempotency-Key: b1" --data-binary "$JSON" $H/v1/edgerouter/apply)" = 200 ] && grep -qi "idempotent-replay: true" $T/hdr'
check "audit lists the writes with the actor, no body, no token" '[ "$(code -H "Authorization: Bearer $RO" "$H/v1/audit?limit=20")" = 200 ] && grep -q "\"actor\":\"ops\"" $T/body && ! grep -q "$ADMIN" $WAYANG_ROOT/data/var/wayang/api-audit.jsonl && ! grep -q "alice@box" $WAYANG_ROOT/data/var/wayang/api-audit.jsonl && [ "$(stat -c %a $WAYANG_ROOT/data/var/wayang/api-audit.jsonl)" = 600 ]'
# SSE: subscribe, make a write, see the event
curl -s -N -H "Authorization: Bearer $RO" $H/v1/events > $T/sse &
C=$!
sleep 1
curl -s -o /dev/null -X POST -H "Authorization: Bearer $ADMIN" --data-binary "nope" $H/v1/ssh-keys
sleep 1; kill $C 2>/dev/null
check "SSE delivers the write as an audit event" 'grep -q "event: audit" $T/sse && grep -q "/v1/ssh-keys" $T/sse'
kill $S; wait $S 2>/dev/null
# read-only (default) server
P2=18633
$B api --listen 127.0.0.1:$P2 --token-file $TF > $T/ro.log 2>&1 &
S2=$!
for _ in $(seq 1 30); do curl -s -o /dev/null http://127.0.0.1:$P2/v1/health && break; sleep 0.2; done
check "the default server is read-only: even admin gets 403 read_only" '[ "$(code -X POST -H "Authorization: Bearer $ADMIN" --data-binary x http://127.0.0.1:$P2/v1/ssh-keys)" = 403 ] && grep -q read_only $T/body'
kill $S2; wait $S2 2>/dev/null
# rate limit
P3=18634
$B api --listen 127.0.0.1:$P3 --token-file $TF --rate-burst 3 --rate-per-sec 0.01 > $T/rl.log 2>&1 &
S3=$!
for _ in $(seq 1 30); do curl -s -o /dev/null http://127.0.0.1:$P3/v1/health && break; sleep 0.2; done
codes=""; for i in 1 2 3 4 5; do codes="$codes $(code -H "Authorization: Bearer $RO" http://127.0.0.1:$P3/v1/ssh-keys)"; done
check "rate limit: the 4th request in the burst is 429 (got:$codes)" 'echo "$codes" | grep -q 429'
kill $S3; wait $S3 2>/dev/null
# guard rails of the flags
check "--rw --insecure-no-auth refused" '$B api --rw --insecure-no-auth 2>&1 | grep -q "needs a bearer token"'
check "non-loopback without TLS refused" '$B api --listen 0.0.0.0:18640 --token-file $TF 2>&1 | grep -q "needs TLS"'
# TLS + mTLS with real certificates
cd $T/tls
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 2 -subj "/CN=test ca" -addext "basicConstraints=critical,CA:TRUE" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout srv.key -out srv.csr -subj "/CN=localhost" >/dev/null 2>&1
printf "subjectAltName=DNS:localhost,IP:127.0.0.1\n" > san.ext
openssl x509 -req -in srv.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out srv.pem -days 2 -extfile san.ext >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout cli.key -out cli.csr -subj "/CN=ops client" >/dev/null 2>&1
printf "basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth\n" > cli.ext
openssl x509 -req -in cli.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out cli.pem -days 2 -extfile cli.ext >/dev/null 2>&1
P4=18635
$B api --listen 127.0.0.1:$P4 --token-file $TF --tls-cert srv.pem --tls-key srv.key > $T/tls.log 2>&1 &
S4=$!
for _ in $(seq 1 30); do curl -s -o /dev/null --cacert ca.pem https://localhost:$P4/v1/health && break; sleep 0.2; done
check "HTTPS with a real certificate: health says tls" '[ "$(code --cacert ca.pem https://localhost:$P4/v1/health)" = 200 ] && grep -q "\"tls\":true" $T/body'
check "HTTPS needs the token as usual" '[ "$(code --cacert ca.pem https://localhost:$P4/v1/ssh-keys)" = 401 ] && [ "$(code --cacert ca.pem -H "Authorization: Bearer $RO" https://localhost:$P4/v1/ssh-keys)" = 200 ]'
check "plain HTTP to the TLS port gets no answer" '! curl -s -m 3 http://127.0.0.1:$P4/v1/health | grep -q "\"ok\":true"'
kill $S4; wait $S4 2>/dev/null
P5=18636
$B api --listen 127.0.0.1:$P5 --token-file $TF --tls-cert srv.pem --tls-key srv.key --client-ca ca.pem > $T/mtls.log 2>&1 &
S5=$!
for _ in $(seq 1 30); do curl -s -o /dev/null --cacert ca.pem --cert cli.pem --key cli.key https://localhost:$P5/v1/health && break; sleep 0.2; done
check "mTLS: the right client certificate + token works" '[ "$(code --cacert ca.pem --cert cli.pem --key cli.key -H "Authorization: Bearer $RO" https://localhost:$P5/v1/ssh-keys)" = 200 ] && true'
check "mTLS: no client certificate, no HTTP at all" '! curl -s -m 5 --cacert ca.pem https://localhost:$P5/v1/health | grep -q "\"ok\":true"'
check "mTLS: the certificate alone is not enough (401 without the token)" '[ "$(code --cacert ca.pem --cert cli.pem --key cli.key https://localhost:$P5/v1/ssh-keys)" = 401 ]'
kill $S5; wait $S5 2>/dev/null
echo; echo "$pass/$((pass+fail)) passed"; [ $fail = 0 ]
