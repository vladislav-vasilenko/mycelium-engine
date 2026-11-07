#!/bin/sh
# Собирает образ dist/Echo-<версия>.dmg для распространения:
# при монтировании Finder показывает Echo.app и ярлык «Applications»,
# установка — перетаскиванием приложения на ярлык.
#
# Использование: scripts/make-dmg.sh [--no-build] [--layout]
#   --no-build  взять готовый target/release/Echo.app, не пересобирать
#   --layout    оформить окно Finder (крупные значки, приложение слева,
#               Applications справа). Нужен доступ терминала к Finder:
#               при первом запуске macOS спросит разрешение (Автоматизация)
#
# Подпись и нотаризация (без них образ откроется только на этом Mac;
# на других Gatekeeper потребует «Открыть» из контекстного меню):
#   ECHO_SIGN_IDENTITY="Developer ID Application: Имя (TEAMID)"
#       подписать приложение с hardened runtime и сам образ
#   ECHO_NOTARY_PROFILE=имя-профиля
#       отправить образ на нотаризацию и прикрепить тикет; профиль создаётся
#       один раз: xcrun notarytool store-credentials имя-профиля
set -eu
cd "$(dirname "$0")/.."

build=1
layout=0
for arg in "$@"; do
    case "$arg" in
        --no-build) build=0 ;;
        --layout) layout=1 ;;
        -h | --help)
            sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "неизвестный аргумент: $arg (см. --help)" >&2
            exit 2
            ;;
    esac
done

app=target/release/Echo.app
if [ "$build" = 1 ]; then
    scripts/bundle-app.sh
fi
if [ ! -d "$app" ]; then
    echo "нет $app: запусти без --no-build" >&2
    exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
volname="Echo $version"
dmg="dist/Echo-$version.dmg"
staging="dist/dmg-staging"

rm -rf "$staging" "$dmg"
mkdir -p "$staging"
ditto "$app" "$staging/Echo.app"
ln -s /Applications "$staging/Applications"

identity="${ECHO_SIGN_IDENTITY:-}"
if [ -n "$identity" ]; then
    # Hardened runtime и доверенная метка времени обязательны для нотаризации.
    codesign --force --options runtime --timestamp --sign "$identity" "$staging/Echo.app"
    codesign --verify --strict --verbose=2 "$staging/Echo.app"
fi

if [ "$layout" = 1 ]; then
    # Записываемый образ → оформление окна в Finder → сжатый образ.
    rw="dist/Echo-rw.dmg"
    rm -f "$rw"
    hdiutil create -quiet -volname "$volname" -srcfolder "$staging" -fs HFS+ -format UDRW -ov "$rw"
    mount=$(hdiutil attach -readwrite -noverify -noautoopen "$rw" | sed -n 's|^/dev/.*\(/Volumes/.*\)$|\1|p' | tail -1)
    osascript <<EOF
tell application "Finder"
    tell disk "$volname"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {200, 120, 760, 460}
        set opts to the icon view options of container window
        set arrangement of opts to not arranged
        set icon size of opts to 128
        set position of item "Echo.app" of container window to {140, 160}
        set position of item "Applications" of container window to {420, 160}
        update without registering applications
        close
    end tell
end tell
EOF
    sync
    hdiutil detach -quiet "$mount"
    hdiutil convert -quiet "$rw" -format UDZO -imagekey zlib-level=9 -o "$dmg"
    rm -f "$rw"
else
    hdiutil create -quiet -volname "$volname" -srcfolder "$staging" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$dmg"
fi
rm -rf "$staging"

if [ -n "$identity" ]; then
    codesign --force --timestamp --sign "$identity" "$dmg"
fi
if [ -n "${ECHO_NOTARY_PROFILE:-}" ]; then
    if [ -z "$identity" ]; then
        echo "нотаризация требует ECHO_SIGN_IDENTITY с сертификатом Developer ID" >&2
        exit 1
    fi
    xcrun notarytool submit "$dmg" --keychain-profile "$ECHO_NOTARY_PROFILE" --wait
    xcrun stapler staple "$dmg"
fi

hdiutil verify -quiet "$dmg"
echo "Готово: $dmg ($(du -h "$dmg" | cut -f1 | tr -d ' '))"
if [ -z "$identity" ]; then
    echo "Подпись ad-hoc: на других Mac образ потребует «Открыть» из контекстного меню. См. --help."
fi
