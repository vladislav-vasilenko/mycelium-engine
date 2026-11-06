#!/bin/sh
# Собирает модульные тесты и создаёт стабильную ссылку target/debug/echo-tests
# на тестовый бинарник с хешем в имени, чтобы отладчик знал путь заранее.
set -eu
cd "$(dirname "$0")/.."
exe=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("profile", {}).get("test") and m.get("executable"):
        print(m["executable"])
' | tail -1)
if [ -z "$exe" ]; then
    echo "тестовый бинарник не найден: запусти cargo test --no-run и проверь ошибки" >&2
    exit 1
fi
ln -sf "$exe" target/debug/echo-tests
echo "target/debug/echo-tests -> $exe"
