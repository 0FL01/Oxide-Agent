#!/bin/bash
exec chromium --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage \
  --remote-debugging-port=9222 \
  --user-data-dir=/home/stfu/ai/Oxide-Agent/.cp0-verify/profile \
  about:blank
