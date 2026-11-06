#!/usr/bin/env bash
# Throwaway: sample where the first Servo session spends its time.
label=$1
BIN=$(ls -t .cache/hypercolor/target/servo/debug/deps/servo_integration_tests-* | grep -v '\.d$' | head -1)
echo "binary $BIN"
start=$(date +%s.%N)
RUST_BACKTRACE=1 "$BIN" --exact servo_renders_canvas2d_page_pixels --nocapture > "run-$label.log" 2>&1 &
PID=$!
sample() {
  sleep "$1"
  kill -0 $PID 2>/dev/null || return 0
  echo "=== sample at $(echo "$(date +%s.%N) - $start" | bc)s"
  top -H -b -n1 -p $PID | sed -n 1,24p
  sudo gdb -p $PID -batch -nx -ex 'set pagination off' -ex 'thread apply all bt 16' 2>/dev/null | grep -E '^(Thread|#)' | cut -c1-150
}
sample 3
sample 6
sample 20
wait $PID
rc=$?
echo "=== exit=$rc after $(echo "$(date +%s.%N) - $start" | bc)s"
cat "run-$label.log"
