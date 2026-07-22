#!/bin/bash
export AETHER_DEBUG=1
cargo run -p aether-shell --features gtk > /tmp/aether_run.log 2>&1 &
APP_PID=$!
sleep 4
kill $APP_PID
cat /tmp/aether_run.log
