#!/bin/sh
# Собирает запускаемый бандл target/release/Echo.app:
#   Contents/MacOS/echo               — release-бинарник
#   Contents/Resources/shaders/metal  — MSL, компилируется при запуске
#   Contents/Info.plist               — из packaging/Info.plist
# Бандл подписывается ad-hoc (`codesign -s -`): запускается на этом Mac,
# для распространения нужна подпись Developer ID и нотаризация.
#
# Использование: scripts/bundle-app.sh [--install] [--open]
#   --install  скопировать бандл в /Applications/Echo.app через ditto
#              (права, подпись и атрибуты сохраняются, карантина нет);
#              путь можно сменить переменной ECHO_INSTALL_DIR
#   --open     запустить приложение (установленное, если указан --install)
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

install=0
open_app=0
for arg in "$@"; do
    case "$arg" in
        --install) install=1 ;;
        --open) open_app=1 ;;
        -h | --help)
            sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "неизвестный аргумент: $arg (см. --help)" >&2
            exit 2
            ;;
    esac
done

cargo build --release

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
build=$(git rev-list --count HEAD 2>/dev/null || echo 1)
app=target/release/Echo.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/shaders"
cp target/release/echo "$app/Contents/MacOS/echo"
cp -R shaders/metal "$app/Contents/Resources/shaders/metal"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" packaging/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict "$app"
echo "Готово: $app (версия $version, сборка $build)"

target="$app"
if [ "$install" = 1 ]; then
    dir="${ECHO_INSTALL_DIR:-/Applications}"
    target="$dir/Echo.app"
    if [ ! -w "$dir" ]; then
        echo "нет прав на запись в $dir; укажи ECHO_INSTALL_DIR=~/Applications" >&2
        exit 1
    fi
    # Закрыть запущенную копию, иначе замена бандла оставит старый процесс.
    pkill -f "$target/Contents/MacOS/echo" 2>/dev/null || true
    rm -rf "$target"
    ditto "$app" "$target"
    xattr -dr com.apple.quarantine "$target" 2>/dev/null || true
    codesign --verify --strict "$target"
    echo "Установлено: $target"
fi

if [ "$open_app" = 1 ]; then
    open "$target"
fi
